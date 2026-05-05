use thiserror::Error;

#[derive(Debug, Error)]
pub enum OperonError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("config: {0}")]
    Config(String),

    #[error("secret: {0}")]
    Secret(String),

    #[error("plugin '{plugin}': {source}")]
    Plugin {
        plugin: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("budget exceeded: {0}")]
    Budget(String),

    #[error("cancelled")]
    Cancelled,

    #[error("provider {provider}: {message}")]
    Provider {
        provider: String,
        message: String,
        retryable: bool,
    },

    #[error("mcp {server}: {message}")]
    Mcp { server: String, message: String },

    #[error("not found: {0}")]
    NotFound(String),
}

pub type OperonResult<T> = Result<T, OperonError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_io() {
        let e = OperonError::Io(std::io::Error::other("oops"));
        assert!(format!("{e}").starts_with("io:"));
    }

    #[test]
    fn display_config() {
        let e = OperonError::Config("bad toml".into());
        assert_eq!(format!("{e}"), "config: bad toml");
    }

    #[test]
    fn display_provider() {
        let e = OperonError::Provider {
            provider: "anthropic".into(),
            message: "401".into(),
            retryable: false,
        };
        assert_eq!(format!("{e}"), "provider anthropic: 401");
    }

    #[test]
    fn plugin_wraps_source() {
        let inner = std::io::Error::other("inner");
        let e = OperonError::Plugin {
            plugin: "test".into(),
            source: Box::new(inner),
        };
        assert!(std::error::Error::source(&e).is_some());
    }

    #[test]
    fn cancelled_displays() {
        assert_eq!(format!("{}", OperonError::Cancelled), "cancelled");
    }

    #[test]
    fn is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OperonError>();
    }
}
