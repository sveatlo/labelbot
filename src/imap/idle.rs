use crate::error::ImapError;
use crate::imap::{ImapSession, fetch};
use async_imap::extensions::idle::IdleResponse;
use futures::StreamExt;
use std::fmt;
use std::sync::Arc;
use tokio::sync::Notify;

pub async fn enter_idle(
    session: ImapSession,
    timeout_secs: u64,
    shutdown: Arc<Notify>,
) -> Result<IdleLoop, ImapError> {
    let mut idle = session.idle();
    idle.init().await?;
    Ok(IdleLoop {
        idle: Some(idle),
        timeout: std::time::Duration::from_secs(timeout_secs),
        shutdown,
    })
}

pub struct IdleLoop {
    idle: Option<
        async_imap::extensions::idle::Handle<tokio_native_tls::TlsStream<tokio::net::TcpStream>>,
    >,
    timeout: std::time::Duration,
    shutdown: Arc<Notify>,
}

#[expect(clippy::missing_fields_in_debug)]
impl fmt::Debug for IdleLoop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdleLoop")
            .field("idle", &self.idle.as_ref().map(|_| "Handle<..>"))
            .field("timeout", &self.timeout)
            .finish()
    }
}

impl IdleLoop {
    /// Wait for the server to send new-data notification (or timeout).
    /// Returns `Some(session)` if we need to process new mail (NewData received),
    /// or `None` if we timed out or shutdown was requested (caller should check shutdown flag).
    pub async fn wait_for_notification(&mut self) -> Result<Option<ImapSession>, ImapError> {
        let mut idle = self.idle.take().expect("idle handle already consumed");

        let idle_response = {
            let (fut, _interrupt) = idle.wait_with_timeout(self.timeout);
            tokio::pin!(fut);
            tokio::select! {
                result = &mut fut => Some(result?),
                () = self.shutdown.notified() => None,
            }
        };

        let session = IdleLoop::complete(idle).await?;

        match idle_response {
            None => Ok(None),
            Some(IdleResponse::NewData(_)) => Ok(Some(session)),
            Some(IdleResponse::Timeout | IdleResponse::ManualInterrupt) => {
                self.idle = Some(session.idle());
                self.idle.as_mut().unwrap().init().await?;
                Ok(None)
            }
        }
    }

    async fn complete(
        idle: async_imap::extensions::idle::Handle<
            tokio_native_tls::TlsStream<tokio::net::TcpStream>,
        >,
    ) -> Result<ImapSession, ImapError> {
        Ok(idle.done().await?)
    }
}

/// Fetch headers of all messages matching the given IMAP search query (e.g. `UNSEEN`).
pub async fn fetch_new_headers(
    session: &mut ImapSession,
    query: &str,
) -> Result<Vec<(u32, fetch::EmailHeaders)>, ImapError> {
    let uids = session.uid_search(query).await?;

    if uids.is_empty() {
        return Ok(Vec::new());
    }

    let uid_list: Vec<String> = uids.iter().map(std::string::ToString::to_string).collect();
    let uid_set = uid_list.join(",");
    let stream = session
        .uid_fetch(uid_set, fetch::HEADER_FETCH_QUERY)
        .await?;

    let mut results = Vec::new();
    tokio::pin!(stream);
    while let Some(fetch) = stream.next().await {
        let f = fetch?;
        if let Some(header_bytes) = f.header()
            && let Some(uid) = f.uid
        {
            let headers = fetch::parse_header_bytes(header_bytes);
            results.push((uid, headers));
        }
    }

    Ok(results)
}
