use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::classifier::LlmClassifier;
use crate::config::Config;
use crate::error::ClassifyError;
use crate::imap;
use crate::imap::idle;
use crate::labels::{IMPORTANT_LABEL, LabelSet};
use crate::store::Store;

pub async fn run(cfg: Config, classifier: Box<dyn LlmClassifier>) -> anyhow::Result<()> {
    let store = Store::connect(&cfg.db_path).await?;
    tracing::info!(db_path = %cfg.db_path, "connected to store");
    let labels = cfg.label_set();

    let shutdown = Arc::new(AtomicBool::new(false));
    {
        let shutdown = shutdown.clone();
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

            shutdown.store(true, Ordering::Relaxed);
        });
    }

    let mut attempt: u32 = 0;
    loop {
        if shutdown.load(Ordering::Relaxed) {
            return Ok(());
        }

        match run_session(&cfg, &labels, classifier.as_ref(), &store, &shutdown).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if shutdown.load(Ordering::Relaxed) {
                    return Ok(());
                }
                attempt = attempt.saturating_add(1);
                let base = 2_u64.saturating_pow(attempt.min(6)).min(60);
                let jitter = rand::random::<u64>() % (base + 1);
                let wait = std::time::Duration::from_secs(base + jitter);
                tracing::warn!(attempt, error=%e, wait_secs=wait.as_secs(), "imap session ended, reconnecting after backoff");
                tokio::time::sleep(wait).await;
            }
        }
    }
}

async fn run_session(
    cfg: &Config,
    labels: &LabelSet,
    classifier: &dyn LlmClassifier,
    store: &Store,
    shutdown: &Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut session = imap::connect(
        &cfg.imap_host,
        cfg.imap_port,
        &cfg.imap_user,
        &cfg.imap_password,
        cfg.tls_insecure,
    )
    .await?;
    tracing::info!("connected to IMAP");

    let created = imap::labels::sync_labels(&mut session, labels).await?;
    if !created.is_empty() {
        tracing::info!(?created, "created label mailboxes");
    }

    session.select("INBOX").await?;
    tracing::info!("selected INBOX");

    if cfg.backfill_max_age_days > 0 {
        backfill(
            &mut session,
            store,
            classifier,
            labels,
            cfg.backfill_max_age_days,
        )
        .await?;
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("shutdown after backfill");
            return Ok(());
        }
    }

    tracing::info!("entering IDLE loop");

    let mut session: Option<imap::ImapSession> = Some(session);

    loop {
        if shutdown.load(Ordering::Relaxed) {
            tracing::info!("shutting down gracefully after last batch");
            break;
        }

        let s = session.take().expect("session should be present");
        let mut idle_loop = idle::enter_idle(s, cfg.poll_idle_timeout_secs).await?;

        session = loop {
            if shutdown.load(Ordering::Relaxed) {
                break None;
            }

            if let Some(new_session) = idle_loop.wait_for_notification().await? {
                break Some(new_session);
            }
            // Timeout — loop back; idle_loop internally re-IDLEs
        };

        let Some(s) = session.as_mut() else {
            tracing::info!("shutting down gracefully after last batch");
            break;
        };

        // New data arrived — fetch UNSEEN messages
        let new_entries = idle::fetch_new_headers(s, "UNSEEN").await?;

        for (uid, headers) in &new_entries {
            if headers.message_id.is_empty() {
                tracing::warn!(%uid, "message without Message-ID, skipping");
                continue;
            }

            if store.is_processed(&headers.message_id).await? {
                tracing::debug!(msg_id = %headers.message_id, "already processed, skipping");
                continue;
            }

            if let Err(e) = classify_and_record(classifier, s, store, labels, *uid, headers).await {
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

            if let Err(e) =
                classify_and_record(classifier, session, store, labels, *uid, headers).await
            {
                tracing::error!(%uid, msg_id=%headers.message_id, error=%e, "backfill classify_and_record failed, continuing");
            }
        }

        tracing::info!(chunk_size = results.len(), "backfill chunk done");
    }

    tracing::info!("backfill complete");
    Ok(())
}

/// Classify an email via the LLM, apply labels to IMAP mailbox, and record in the
/// deduplication store. Non-fatal store errors are logged but not propagated.
async fn classify_and_record(
    classifier: &dyn LlmClassifier,
    session: &mut imap::ImapSession,
    store: &Store,
    label_set: &LabelSet,
    uid: u32,
    headers: &imap::fetch::EmailHeaders,
) -> Result<(), anyhow::Error> {
    match classifier.classify(&headers.subject, &headers.from).await {
        Ok(mut labels) => {
            if labels.iter().any(|l| label_set.is_important(l))
                && !labels.iter().any(|l| l == IMPORTANT_LABEL)
            {
                labels.push(IMPORTANT_LABEL.to_owned());
            }

            if !labels.is_empty() {
                imap::labels::apply_labels(session, uid, &labels).await?;
            }

            let label_str = labels.join(",");
            tracing::info!(msg_id = %headers.message_id, labels = %label_str, "classified");
            if let Err(e) = store.record(&headers.message_id, &label_str).await {
                tracing::error!(msg_id = %headers.message_id, error = %e, "store record failed");
            }
        }
        Err(ClassifyError::RateLimited { .. }) => {
            tracing::warn!("rate limited, pausing classification");
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
        Err(e @ (ClassifyError::Parse(_) | ClassifyError::Unimplemented(_))) => {
            // Terminal: classifier produced an unusable response. Record so we
            // don't loop on the same message forever.
            tracing::error!(msg_id = %headers.message_id, error = %e, "classification permanently failed, marking processed");
            if let Err(err) = store.record(&headers.message_id, "").await {
                tracing::error!(msg_id = %headers.message_id, error = %err, "store record failed");
            }
        }
        Err(e) => {
            // Transient (HTTP, 5xx, transport): do NOT record, retry on next pass.
            tracing::warn!(msg_id = %headers.message_id, error = %e, "classification transient failure, will retry");
        }
    }

    Ok(())
}
