use crate::provider::lmstudio::LmStudioClient;
use crate::provider::ollama::OllamaClient;
use async_trait::async_trait;
use clap::ValueEnum;
use futures::Stream;
use std::fmt::{Display, Formatter};
use std::pin::Pin;

pub mod lmstudio;
pub mod ollama;

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum LlmProvider {
    Ollama,
    LmStudio,
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
pub(crate) trait LlmClient {
    async fn generate(&self, model: &str, prompt: &str) -> anyhow::Result<String>;
    async fn generate_stream(&self, model: &str, prompt: &str) -> anyhow::Result<LlmStream>;
}

pub(crate) fn create_client(provider: LlmProvider) -> Box<dyn LlmClient> {
    match provider {
        LlmProvider::Ollama => Box::new(OllamaClient::new()),
        LlmProvider::LmStudio => Box::new(LmStudioClient::new()),
    }
}
