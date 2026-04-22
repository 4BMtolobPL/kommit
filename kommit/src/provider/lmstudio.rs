use crate::provider::LlmClient;
use async_trait::async_trait;
use lms_api::LmStudio;
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

        // TODO: let res = lms.chat()

        Ok("Dev".to_string())
    }
}
