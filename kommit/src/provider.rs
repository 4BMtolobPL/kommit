use crate::provider::lmstudio::LmStudioClient;
use crate::provider::ollama::OllamaClient;
use async_trait::async_trait;
use clap::ValueEnum;
use futures::Stream;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};
use std::pin::Pin;
use url::Url;

pub mod lmstudio;
pub mod ollama;

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LlmProvider {
    Ollama,
    LmStudio,
}

impl LlmProvider {
    pub fn default_host(&self) -> Url {
        match self {
            LlmProvider::Ollama => Url::parse("http://localhost").unwrap(),
            LlmProvider::LmStudio => Url::parse("http://localhost").unwrap(),
        }
    }

    pub fn default_port(&self) -> u16 {
        match self {
            LlmProvider::Ollama => 11434,
            LlmProvider::LmStudio => 1234,
        }
    }
}

impl Display for LlmProvider {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_possible_value().unwrap().get_name())
    }
}

pub(crate) enum StreamResponse {
    Think(String),
    Generate(String),
}

pub(crate) type LlmStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamResponse>> + Send>>;

#[async_trait]
pub(crate) trait ProviderStrategy {
    async fn generate(
        &self,
        model: &str,
        prompt: &str,
        think: Option<ThinkType>,
    ) -> anyhow::Result<String>;
    async fn generate_stream(
        &self,
        model: &str,
        prompt: &str,
        think: Option<ThinkType>,
    ) -> anyhow::Result<LlmStream>;
}

pub(crate) fn create_client(
    provider: LlmProvider,
    host: Url,
    port: u16,
) -> anyhow::Result<Box<dyn ProviderStrategy>> {
    match provider {
        LlmProvider::Ollama => Ok(Box::new(OllamaClient::new(host, port)?)),
        LlmProvider::LmStudio => Ok(Box::new(LmStudioClient::new(host, port)?)),
    }
}

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ThinkType {
    True,
    False,
    Low,
    Medium,
    High,
}

impl Display for ThinkType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_possible_value().unwrap().get_name())
    }
}
