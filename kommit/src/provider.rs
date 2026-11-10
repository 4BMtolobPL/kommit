use crate::provider::lmstudio::LmStudioClient;
use crate::provider::ollama::OllamaClient;
use async_trait::async_trait;
use clap::ValueEnum;
use futures::Stream;
use indicatif::ProgressStyle;
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
    #[serde(rename = "lm-studio", alias = "lmstudio")]
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
    ThinkDone,
    Generate(String),
}

pub(crate) type LlmStream = Pin<Box<dyn Stream<Item = anyhow::Result<StreamResponse>> + Send>>;

#[async_trait]
pub(crate) trait ProviderStrategy {
    /// The provider's name as users know it, e.g. "LM Studio".
    fn name(&self) -> &'static str;
    /// The command a user would run to download `model` themselves.
    fn download_command(&self, model: &str) -> String;
    /// Whether `model` is installed, without downloading or loading it.
    async fn is_model_installed(&self, model: &str) -> anyhow::Result<bool>;
    /// Downloads `model`, reporting progress on stderr.
    async fn download_model(&self, model: &str) -> anyhow::Result<()>;
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

/// The style of a download progress bar. Bars are drawn on stderr, and only on a terminal.
pub(crate) fn download_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{msg:<20}: {percent:>3}% {bar:40} | {bytes:>11}/{total_bytes:11} {binary_bytes_per_sec:13} {eta}",
    )
    .expect("valid progress template")
}

/// The error for a model that is not installed, naming every way to get it.
pub(crate) fn missing_model_error(provider: &str, model: &str, command: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Model '{model}' was not found in {provider}. Pass --pull to download it, download it first (e.g. `{command}`), or check the model name."
    )
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
