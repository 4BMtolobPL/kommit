pub mod error;
pub mod models;

use reqwest::{Client, IntoUrl, Url};
use std::time::Duration;

pub struct LmStudio {
    url: Url,
    client: Client,
}

impl LmStudio {
    pub fn new(host: impl IntoUrl, port: u16) -> Self {
        let mut url = host.into_url().unwrap();

        url.set_port(Some(port)).unwrap();

        Self::from_url(url)
    }

    fn from_url(url: Url) -> Self {
        Self {
            url,
            ..Default::default()
        }
    }
}

impl Default for LmStudio {
    fn default() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap();

        Self {
            url: Url::parse("http://localhost:1234").unwrap(),
            client,
        }
    }
}
