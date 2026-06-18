use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;

use crate::classifier::{ClassifyOutcome, LlmClassifier};
use crate::config::Config;
use crate::imap;
use crate::imap::idle;
use crate::labels::LabelSet;
use crate::store::Store;
use crate::summarizer::Summarizer;

pub async fn run(
    cfg: Config,
    classifier: Box<dyn LlmClassifier>,
    summarizer: Box<dyn Summarizer>,
) -> anyhow::Result<()> {
    let store = Store::connect(&cfg.db_path).await?;
    tracing::info!(db_path = %cfg.db_path, "connected to store");
    let labels = cfg.label_set();

    let token = CancellationToken::new();
    {
        let token = token.clone();
        tokio::spawn(async move {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("register SIGTERM handler");
            let mut sigint =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                    .expect("register SIGINT handler");

            tokio::select! {
                _ = sigterm.recv() => tracing::info!("received SIGTERM"),
                _ = sigint.recv() => tracing::info!("received SIGINT"),
            }

            token.cancel();
        });
    }

    let mut attempt: u32 = 0;
    loop {
        if token.is_cancelled() {
            return Ok(());
        }

        match run_session(
            &cfg,
            &labels,
            classifier.as_ref(),
            summarizer.as_ref(),
            &store,
            token.clone(),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(e) => {
                if token.is_cancelled() {
                    return Ok(());
                }
                attempt = attempt.saturating_add(1);
                let base = 2_u64.saturating_pow(attempt.min(6)).min(60);
                let jitter = rand::random::<u64>() % (base + 1);
                let wait = std::time::Duration::from_secs(base + jitter);
                tracing::warn!(attempt, error=%e, wait_secs=wait.as_secs(), "imap session ended, reconnecting after backoff");
                tokio::select! {
                    () = tokio::time::sleep(wait) => {},
                    () = token.cancelled() => return Ok(()),
                }
            }
        }
    }
}

async fn run_session(
    cfg: &Config,
    labels: &LabelSet,
    classifier: &dyn LlmClassifier,
    summarizer: &dyn Summarizer,
    store: &Store,
    cancellation_token: CancellationToken,
) -> anyhow::Result<()> {
    let mut session = imap::connect(
        &cfg.imap.host,
        cfg.imap.port,
        &cfg.imap.user,
        &cfg.imap.password,
        cfg.imap.tls_insecure,
    )
    .await?;
    tracing::info!("connected to IMAP");

    let created = imap::labels::sync_labels(&mut session, labels).await?;
    if !created.is_empty() {
        tracing::info!(?created, "created label mailboxes");
    }

    session.select("INBOX").await?;
    tracing::debug!("selected INBOX");

    if cfg.backfill_max_age_days > 0 {
        backfill(
            &mut session,
            store,
            classifier,
            summarizer,
            labels,
            cfg.backfill_max_age_days,
        )
        .await?;
        if cancellation_token.is_cancelled() {
            tracing::info!("shutdown after backfill");
            return Ok(());
        }
    }

    tracing::info!("entering IDLE loop");

    let mut session: imap::ImapSession = session;

    'outer: loop {
        if cancellation_token.is_cancelled() {
            tracing::info!("shutting down gracefully after last batch");
            break;
        }

        let mut idle_loop = idle::enter_idle(
            session,
            cfg.poll_idle_timeout_secs,
            cancellation_token.clone(),
        )
        .await?;

        session = loop {
            let Some(res) = idle_loop
                .wait_for_notification()
                .with_cancellation_token(&cancellation_token)
                .await
            else {
                break 'outer;
            };

            if let Some(new_session) = res? {
                break new_session;
            }
            // Timeout — loop back; idle_loop internally re-IDLEs
        };

        // New data arrived — fetch UNSEEN messages
        let new_entries = idle::fetch_new_headers(&mut session, "UNSEEN").await?;

        for (uid, headers) in &new_entries {
            if headers.message_id.is_empty() {
                tracing::warn!(%uid, "message without Message-ID, skipping");
                continue;
            }

            if store.is_processed(&headers.message_id).await? {
                tracing::debug!(msg_id = %headers.message_id, "already processed, skipping");
                continue;
            }

            if let Err(e) = classify_and_record(
                classifier,
                summarizer,
                &mut session,
                store,
                labels,
                *uid,
                headers,
            )
            .await
            {
                tracing::error!(%uid, msg_id=%headers.message_id, error=%e, "classify_and_record failed, continuing");
            }
        }
    }

    Ok(())
}

async fn backfill(
    session: &mut imap::ImapSession,
    store: &Store,
    classifier: &dyn LlmClassifier,
    summarizer: &dyn Summarizer,
    labels: &LabelSet,
    max_age_days: u64,
) -> anyhow::Result<()> {
    use chrono::Utc;

    #[expect(clippy::cast_possible_wrap)]
    let days = max_age_days as i64;
    let since = Utc::now() - chrono::Duration::days(days);
    let since_str = since.format("%d-%b-%Y").to_string();

    let search_query = format!("SINCE {since_str}");
    let uids = session.uid_search(&search_query).await?;

    tracing::info!(count = uids.len(), "backfill processing");

    let uid_set: Vec<String> = uids.iter().map(std::string::ToString::to_string).collect();
    for chunk in uid_set.chunks(100) {
        let query = chunk.join(",");
        let stream = session
            .uid_fetch(query, imap::fetch::HEADER_FETCH_QUERY)
            .await?;

        let results: Vec<(u32, imap::fetch::EmailHeaders)> = {
            use futures::StreamExt;
            tokio::pin!(stream);
            let mut acc = Vec::new();
            while let Some(f) = stream.next().await {
                let f = f?;
                if let Some(hb) = f.header()
                    && let Some(uid) = f.uid
                {
                    acc.push((uid, imap::fetch::parse_header_bytes(hb)));
                }
            }
            acc
        };

        for (uid, headers) in &results {
            if headers.message_id.is_empty() {
                continue;
            }
            if store.is_processed(&headers.message_id).await? {
                continue;
            }

            if let Err(e) = classify_and_record(
                classifier, summarizer, session, store, labels, *uid, headers,
            )
            .await
            {
                tracing::error!(%uid, msg_id=%headers.message_id, error=%e, "backfill classify_and_record failed, continuing");
            }
        }

        tracing::info!(chunk_size = results.len(), "backfill chunk done");
    }

    tracing::info!("backfill complete");
    Ok(())
}

/// Fetch and summarise the message body. Returns `None` on any failure so
/// classification can proceed without a body rather than aborting.
async fn fetch_and_summarize(
    session: &mut imap::ImapSession,
    summarizer: &dyn Summarizer,
    uid: u32,
) -> Option<String> {
    match imap::fetch::fetch_message_body(session, uid).await {
        Ok(Some(body)) => match summarizer.summarize(&body).await {
            Ok(s) => {
                tracing::debug!(%uid, summary_len = s.len(), "body summarized for classification");
                Some(s)
            }
            Err(e) => {
                tracing::warn!(%uid, error=%e, "summarization failed, proceeding without body");
                None
            }
        },
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(%uid, error=%e, "body fetch failed, proceeding without body");
            None
        }
    }
}

/// Pure classification decision: call the classifier and augment the result
/// with the importance label if applicable. No I/O — testable with a mock.
async fn decide_labels(
    classifier: &dyn LlmClassifier,
    label_set: &LabelSet,
    subject: &str,
    from_addr: &str,
    body_summary: Option<&str>,
) -> ClassifyOutcome {
    match classifier.classify(subject, from_addr, body_summary).await {
        ClassifyOutcome::Labels(labels) => ClassifyOutcome::Labels(label_set.augment(labels)),
        other => other,
    }
}

/// Classify an email, apply labels to IMAP, and record in the dedup store.
/// Non-fatal store errors are logged but not propagated.
async fn classify_and_record(
    classifier: &dyn LlmClassifier,
    summarizer: &dyn Summarizer,
    session: &mut imap::ImapSession,
    store: &Store,
    label_set: &LabelSet,
    uid: u32,
    headers: &imap::fetch::EmailHeaders,
) -> Result<(), anyhow::Error> {
    let body_summary = fetch_and_summarize(session, summarizer, uid).await;

    match decide_labels(
        classifier,
        label_set,
        &headers.subject,
        &headers.from,
        body_summary.as_deref(),
    )
    .await
    {
        ClassifyOutcome::Labels(labels) => {
            if !labels.is_empty() {
                imap::labels::apply_labels(session, uid, &labels).await?;
            }
            let label_str = labels.join(",");
            tracing::info!(msg_id = %headers.message_id, labels = %label_str, "classified");
            if let Err(e) = store.record(&headers.message_id, &label_str).await {
                tracing::error!(msg_id = %headers.message_id, error = %e, "store record failed");
            }
        }
        ClassifyOutcome::Terminal => {
            tracing::error!(
                msg_id = %headers.message_id,
                "classification permanently failed, marking processed"
            );
            if let Err(e) = store.record(&headers.message_id, "").await {
                tracing::error!(msg_id = %headers.message_id, error = %e, "store record failed");
            }
        }
        ClassifyOutcome::Transient => {
            tracing::warn!(
                msg_id = %headers.message_id,
                "classification transient failure, will retry"
            );
        }
    }

    Ok(())
}
