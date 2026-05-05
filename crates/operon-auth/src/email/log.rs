use async_trait::async_trait;

use crate::email::EmailSender;
use crate::error::AuthError;

/// Writes emails to `tracing::info!` instead of sending. Used in tests + dev.
#[derive(Default, Clone)]
pub struct LogEmailSender;

#[async_trait]
impl EmailSender for LogEmailSender {
    async fn send(
        &self,
        to: &str,
        subject: &str,
        _body_html: &str,
        body_text: &str,
    ) -> Result<(), AuthError> {
        tracing::info!(target: "email", to = to, subject = subject, body = body_text, "email_sent");
        Ok(())
    }
}
