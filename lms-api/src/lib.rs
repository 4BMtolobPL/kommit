pub mod chat;
pub mod error;
pub mod models;
pub mod types;

use crate::error::ApiError;
use reqwest::{Client, IntoUrl, Response, Url};
use serde::de::DeserializeOwned;
use std::time::Duration;

pub struct LmStudio {
    url: Url,
    client: Client,
}

impl LmStudio {
    pub fn new(host: impl IntoUrl, port: u16) -> Result<Self, ApiError> {
        let mut url = host
            .into_url()
            .map_err(|e| ApiError::Config(e.to_string()))?;

        url.set_port(Some(port))
            .map_err(|_| ApiError::Config("Could not set port".to_string()))?;

        Ok(Self::from_url(url))
    }

    pub fn from_url(url: Url) -> Self {
        let client = Client::builder().build().unwrap_or_default();

        Self { url, client }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.client = Client::builder()
            .timeout(timeout)
            .build()
            .unwrap_or_default();
        self
    }

    pub fn with_client(mut self, client: Client) -> Self {
        self.client = client;
        self
    }

    async fn handle_response<T: DeserializeOwned>(&self, res: Response) -> Result<T, ApiError> {
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        Ok(res.json::<T>().await?)
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApiError> {
        self.url.join(path).map_err(ApiError::Url)
    }
}

impl Default for LmStudio {
    fn default() -> Self {
        Self::from_url(Url::parse("http://localhost:1234").expect("Valid default URL"))
    }
}
