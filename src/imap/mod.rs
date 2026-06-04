pub mod fetch;
pub mod labels;
pub mod idle;

use crate::error::ImapError;
use async_imap::Session;
use tokio::net::TcpStream;

pub type ImapSession = Session<tokio_native_tls::TlsStream<TcpStream>>;

pub async fn connect(
    host: &str,
    port: u16,
    username: &str,
    password: &str,
    tls_insecure: bool,
) -> Result<ImapSession, ImapError> {
    let tcp = TcpStream::connect((host, port)).await?;

    let mut client = async_imap::Client::new(tcp);
    client.run_command_and_check_ok("STARTTLS", None).await?;
    let tcp = client.into_inner();

    let native_connector = if tls_insecure {
        native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true)
            .build()?
    } else {
        native_tls::TlsConnector::new()?
    };
    let connector = tokio_native_tls::TlsConnector::from(native_connector);
    let tls_stream = connector
        .connect(host, tcp)
        .await?;

    let client = async_imap::Client::new(tls_stream);
    let session = client.login(username, password).await.map_err(|(err, _)| err)?;

    Ok(session)
}
