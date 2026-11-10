pub use reqwest::StatusCode;

/// An error from talking to the LM Studio API.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    /// The server could not be reached, or its response could not be read.
    #[error("request failed: {0}")]
    Http(#[from] reqwest::Error),

    /// The server answered with a non-success status.
    #[error("server returned {status}: {body}")]
    Server { status: StatusCode, body: String },

    /// A streamed event could not be parsed.
    #[error("invalid stream event: {0}")]
    Decode(#[from] serde_json::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("invalid configuration: {0}")]
    Config(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
