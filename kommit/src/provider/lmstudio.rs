use crate::provider::LlmClient;
use anyhow::Context;
use async_trait::async_trait;
use lms_api::LmStudio;
use lms_api::chat::request::ChatRequestBuilder;
use lms_api::chat::response::Output;
use std::fmt::Write;
use tracing::{info, instrument, trace};

pub(crate) struct LmStudioClient {}

impl LmStudioClient {
    pub(crate) fn new() -> Self {
        Self {}
    }
}

#[async_trait]
impl LlmClient for LmStudioClient {
    #[instrument(skip(self, model, prompt))]
    async fn generate(&self, model: &str, prompt: &str) -> anyhow::Result<String> {
        info!(%model, "Generating message");
        trace!(model = model, prompt = prompt, "Generating commit message");

        let lms = LmStudio::default();

        let res = lms
            .chat(
                ChatRequestBuilder::default()
                    .model(model)
                    .input(prompt)
                    .build()?,
            )
            .await
            .context("Failed to connect to LmStudio. Is it running?")?;

        let output = res.output;

        let mut s = String::new();
        for o in output {
            match o {
                Output::Message { content } => {
                    write!(s, "{content}")?;
                }
                Output::Reasoning { content } => {
                    info!(content = %content, "Reasoning");
                }
                Output::ToolCall { .. } | Output::InvalidToolCall { .. } => continue,
            }
        }

        Ok(s)
    }
}
