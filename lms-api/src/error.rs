use reqwest::StatusCode;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("unexpected status: {0}")]
    Status(StatusCode, String),

    #[error("unexpected error: {0}")]
    Other(String),
}
