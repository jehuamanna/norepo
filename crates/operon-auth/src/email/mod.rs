use async_trait::async_trait;

use crate::error::AuthError;

pub mod log;
pub mod smtp;

pub use log::LogEmailSender;
pub use smtp::SmtpEmailSender;

#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send(
        &self,
        to: &str,
        subject: &str,
        body_html: &str,
        body_text: &str,
    ) -> Result<(), AuthError>;
}
