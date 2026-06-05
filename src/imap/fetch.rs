/// Fetch query string for header-only retrieval.
/// Requests Message-ID, Subject, and From headers without marking as read.
pub const HEADER_FETCH_QUERY: &str = "(UID BODY.PEEK[HEADER.FIELDS (MESSAGE-ID SUBJECT FROM)])";

/// Fetch the plain-text body of a message by UID. Returns `None` if not found or no text part.
pub async fn fetch_message_body(
    session: &mut super::ImapSession,
    uid: u32,
) -> anyhow::Result<Option<String>> {
    use futures::StreamExt as _;

    let stream = session
        .uid_fetch(uid.to_string(), "(UID BODY.PEEK[])")
        .await?;
    tokio::pin!(stream);

    while let Some(msg) = stream.next().await {
        let msg = msg?;
        if let Some(raw) = msg.body() {
            let parsed =
                mailparse::parse_mail(raw).map_err(|e| anyhow::anyhow!("parse mail: {e}"))?;
            let text = extract_text_body(&parsed);
            tracing::debug!(uid, text_len = text.len(), "fetched message body");
            if !text.is_empty() {
                return Ok(Some(text));
            }
        } else {
            tracing::debug!(uid, "BODY.PEEK[] fetch returned no body data");
        }
    }

    tracing::debug!(uid, "no usable text body found in message");
    Ok(None)
}

/// Extract text content: prefer text/plain, fall back to stripped text/html.
fn extract_text_body(mail: &mailparse::ParsedMail<'_>) -> String {
    let mime = mail.ctype.mimetype.to_ascii_lowercase();

    if mime == "text/plain" {
        return mail.get_body().unwrap_or_default();
    }

    if mime == "text/html" {
        return crate::util::strip_html_tags(&mail.get_body().unwrap_or_default());
    }

    if mime.starts_with("multipart/") {
        // First pass: look for text/plain
        for part in &mail.subparts {
            if part.ctype.mimetype.eq_ignore_ascii_case("text/plain") {
                let text = part.get_body().unwrap_or_default();
                if !text.is_empty() {
                    return text;
                }
            }
        }
        // Second pass: recurse (catches nested multipart and text/html fallback)
        for part in &mail.subparts {
            let text = extract_text_body(part);
            if !text.is_empty() {
                return text;
            }
        }
    }

    String::new()
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailHeaders {
    pub message_id: String,
    pub subject: String,
    pub from: String,
}

/// Parse raw header bytes (as returned by IMAP FETCH BODY.PEEK[HEADER.FIELDS ...])
/// into structured headers. The input is a raw RFC 5322 header block containing
/// only the requested header lines.
pub fn parse_header_bytes(raw: &[u8]) -> EmailHeaders {
    let Ok((headers, _)) = mailparse::parse_headers(raw) else {
        return EmailHeaders {
            message_id: String::new(),
            subject: String::new(),
            from: String::new(),
        };
    };

    let find = |name: &str| -> String {
        headers
            .iter()
            .find(|h| h.get_key_ref().eq_ignore_ascii_case(name))
            .map(mailparse::MailHeader::get_value)
            .unwrap_or_default()
    };

    EmailHeaders {
        message_id: find("Message-ID"),
        subject: find("Subject"),
        from: find("From"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_fixture(subject: &str, from: &str, msg_id: &str) -> Vec<u8> {
        format!("MESSAGE-ID: {msg_id}\r\nSUBJECT: {subject}\r\nFROM: {from}\r\n\r\n").into_bytes()
    }

    #[test]
    fn parses_all_headers() {
        let raw = raw_fixture("Hello World", "alice@example.com", "<abc123@local>");
        let h = parse_header_bytes(&raw);
        assert_eq!(h.message_id, "<abc123@local>");
        assert_eq!(h.subject, "Hello World");
        assert_eq!(h.from, "alice@example.com");
    }

    #[test]
    fn handles_missing_message_id() {
        let raw = b"SUBJECT: Test\r\nFROM: bob@x.com\r\n";
        let h = parse_header_bytes(raw);
        assert!(h.message_id.is_empty());
        assert_eq!(h.subject, "Test");
        assert_eq!(h.from, "bob@x.com");
    }

    #[test]
    fn handles_folded_headers() {
        let raw = b"SUBJECT: Very long\r\n subject that\r\n wraps\r\nFROM: test@x.com\r\nMESSAGE-ID: <m1>\r\n";
        let h = parse_header_bytes(raw);
        assert_eq!(h.subject, "Very long subject that wraps");
    }

    #[test]
    fn from_value_containing_colon_does_not_poison_subject() {
        let raw = b"From: \"Subject: weird\" <weird@example.com>\r\nSubject: Real Subject\r\nMessage-ID: <m1>\r\n";
        let h = parse_header_bytes(raw);
        assert_eq!(h.subject, "Real Subject");
        assert_eq!(h.from, "\"Subject: weird\" <weird@example.com>");
        assert_eq!(h.message_id, "<m1>");
    }

    #[test]
    fn decodes_rfc2047_subject() {
        let raw = b"Subject: =?UTF-8?B?SGVsbG8sIFdvcmxkIQ==?=\r\nFrom: a@b\r\nMessage-ID: <x>\r\n";
        let h = parse_header_bytes(raw);
        assert_eq!(h.subject, "Hello, World!");
    }

    #[test]
    fn header_fetch_query_is_correct() {
        assert!(HEADER_FETCH_QUERY.contains("BODY.PEEK[HEADER.FIELDS"));
        assert!(HEADER_FETCH_QUERY.contains("MESSAGE-ID"));
        assert!(HEADER_FETCH_QUERY.contains("SUBJECT"));
        assert!(HEADER_FETCH_QUERY.contains("FROM"));
        assert!(HEADER_FETCH_QUERY.contains("UID"));
    }
}
