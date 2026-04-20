use thiserror::Error;

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("unexpected status: {0}")]
    Status(reqwest::StatusCode),
}
