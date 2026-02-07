//! HTTP client errors for the network tool layer.

use std::fmt;

/// Error type for HTTP client operations.
#[derive(Debug)]
pub struct HttpError {
    pub message: String,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for HttpError {}

impl From<reqwest::Error> for HttpError {
    fn from(e: reqwest::Error) -> Self {
        Self {
            message: e.to_string(),
        }
    }
}
