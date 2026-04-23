use crate::provider::lmstudio::LmStudioClient;
use crate::provider::ollama::OllamaClient;
use async_trait::async_trait;
use clap::ValueEnum;
use std::fmt::{Display, Formatter};

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

#[async_trait]
pub(crate) trait LlmClient {
    async fn generate(&self, model: &str, prompt: &str) -> anyhow::Result<String>;
}

pub(crate) fn create_client(provider: LlmProvider) -> Box<dyn LlmClient> {
    match provider {
        LlmProvider::Ollama => Box::new(OllamaClient::new()),
        LlmProvider::LmStudio => Box::new(LmStudioClient::new()),
    }
}
