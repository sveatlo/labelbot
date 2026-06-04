use crate::error::ImapError;
use crate::imap::ImapSession;
use crate::labels::{IMPORTANT_LABEL, LabelSet};
use futures::StreamExt;

const LABEL_PREFIX: &str = "Labels";

/// Build the IMAP mailbox name for a label (e.g. "Work" -> "Labels/Work").
pub fn label_mailbox_name(label: &str) -> String {
    format!("{LABEL_PREFIX}/{label}")
}

/// Ensure every configured label mailbox (plus the "Important" auxiliary, if
/// any label is marked important) exists on the server. Returns the list of
/// newly-created mailbox names.
pub async fn sync_labels(
    session: &mut ImapSession,
    labels: &LabelSet,
) -> Result<Vec<String>, ImapError> {
    let existing = list_labels(session).await?;

    let mut wanted: Vec<String> = labels.names().iter().cloned().collect();
    if labels.has_any_important() {
        wanted.push(IMPORTANT_LABEL.to_owned());
    }

    let mut created = Vec::new();
    for label in &wanted {
        let expected = label_mailbox_name(label);
        if !existing.iter().any(|n| n == &expected) {
            create_label_mailbox(session, &expected).await?;
            created.push(expected);
        }
    }

    Ok(created)
}

/// Apply the given labels to an email by UID via UID COPY into each label
/// mailbox.
pub async fn apply_labels(
    session: &mut ImapSession,
    uid: u32,
    labels: &[String],
) -> Result<(), ImapError> {
    for label in labels {
        let mbox = label_mailbox_name(label);
        session
            .uid_copy(uid.to_string(), &mbox)
            .await
            .map_err(|e| {
                tracing::warn!(%uid, mailbox=%mbox, error=%e, "uid_copy failed for label");
                ImapError::LabelMailbox(mbox.clone())
            })?;
        tracing::debug!(%uid, mailbox=%mbox, "applied label");
    }
    Ok(())
}

async fn list_labels(session: &mut ImapSession) -> Result<Vec<String>, ImapError> {
    let mailboxes = session
        .list(Some(&format!("{LABEL_PREFIX}/")), Some("*"))
        .await?;

    let names: Vec<String> = mailboxes
        .filter_map(|mb| async move {
            match mb {
                Ok(ref name) => {
                    let n = name.name();
                    if n.is_empty() { None } else { Some(n.to_owned()) }
                }
                _ => None,
            }
        })
        .collect()
        .await;

    Ok(names)
}

async fn create_label_mailbox(
    session: &mut ImapSession,
    mailbox: &str,
) -> Result<(), ImapError> {
    session
        .run_command_and_check_ok(format!(r#"CREATE "{mailbox}""#))
        .await?;

    tracing::info!(mailbox=%mailbox, "created label mailbox");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_mailbox_name_maps_correctly() {
        assert_eq!(label_mailbox_name("Work"), "Labels/Work");
        assert_eq!(label_mailbox_name("Family"), "Labels/Family");
        assert_eq!(label_mailbox_name(IMPORTANT_LABEL), "Labels/Important");
    }
}
