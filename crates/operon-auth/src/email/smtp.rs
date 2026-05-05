use async_trait::async_trait;
use lettre::message::header::ContentType;
use lettre::message::{MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::email::EmailSender;
use crate::error::AuthError;

/// SMTP-backed email sender. Configuration via env: OPN_SMTP_HOST,
/// OPN_SMTP_PORT, OPN_SMTP_USERNAME, OPN_SMTP_PASSWORD, OPN_SMTP_FROM.
#[derive(Clone)]
pub struct SmtpEmailSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: String,
}

impl SmtpEmailSender {
    pub fn from_env() -> Result<Self, AuthError> {
        let host = std::env::var("OPN_SMTP_HOST")
            .map_err(|_| AuthError::Email("OPN_SMTP_HOST not set".into()))?;
        let port = std::env::var("OPN_SMTP_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(587u16);
        let username = std::env::var("OPN_SMTP_USERNAME")
            .map_err(|_| AuthError::Email("OPN_SMTP_USERNAME not set".into()))?;
        let password = std::env::var("OPN_SMTP_PASSWORD")
            .map_err(|_| AuthError::Email("OPN_SMTP_PASSWORD not set".into()))?;
        let from = std::env::var("OPN_SMTP_FROM").unwrap_or_else(|_| username.clone());

        let creds = Credentials::new(username, password);
        let transport = AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
            .map_err(|e| AuthError::Email(e.to_string()))?
            .port(port)
            .credentials(creds)
            .build();
        Ok(Self { transport, from })
    }
}

#[async_trait]
impl EmailSender for SmtpEmailSender {
    async fn send(
        &self,
        to: &str,
        subject: &str,
        body_html: &str,
        body_text: &str,
    ) -> Result<(), AuthError> {
        let msg = Message::builder()
            .from(
                self.from
                    .parse()
                    .map_err(|e: lettre::address::AddressError| AuthError::Email(e.to_string()))?,
            )
            .to(to
                .parse()
                .map_err(|e: lettre::address::AddressError| AuthError::Email(e.to_string()))?)
            .subject(subject)
            .multipart(
                MultiPart::alternative()
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_PLAIN)
                            .body(body_text.to_string()),
                    )
                    .singlepart(
                        SinglePart::builder()
                            .header(ContentType::TEXT_HTML)
                            .body(body_html.to_string()),
                    ),
            )
            .map_err(|e| AuthError::Email(e.to_string()))?;
        self.transport
            .send(msg)
            .await
            .map_err(|e| AuthError::Email(e.to_string()))?;
        Ok(())
    }
}
