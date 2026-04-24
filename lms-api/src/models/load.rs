use crate::LmStudio;
use crate::error::ApiError;
use crate::models::load::request::LoadRequest;
use crate::models::load::response::LoadResponse;
use tracing::{info, instrument};

impl LmStudio {
    #[instrument(skip(self, request), fields(url = %self.url, endpoint = "/api/v1/models/load", model = %request.model))]
    pub async fn load(&self, request: LoadRequest) -> Result<LoadResponse, ApiError> {
        info!("Load an LLM or Embedding model into memory with custom configuration for inference");

        let url = self.endpoint("api/v1/models/load")?;
        let res = self.client.post(url).json(&request).send().await?;

        self.handle_response(res).await
    }
}

pub mod request {
    use derive_builder::Builder;
    use serde::Serialize;

    #[derive(Serialize, Builder)]
    #[builder(setter(into, strip_option))]
    pub struct LoadRequest {
        pub(super) model: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        context_length: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        eval_batch_size: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        flash_attention: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        num_experts: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        offload_kv_cache_to_gpu: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        echo_load_config: Option<bool>,
    }
}

pub mod response {
    use crate::types::{LoadStatus, ModelType};
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct LoadResponse {
        #[serde(rename = "type")]
        pub model_type: ModelType,
        pub instance_id: String,
        pub load_time_seconds: f64,
        pub status: LoadStatus,
        pub load_config: Option<LoadConfig>,
    }

    #[derive(Deserialize, Debug)]
    pub enum LoadConfig {
        LLMLoadConfig(LLMLoadConfig),
        EmbeddingModelLoadConfig(EmbeddingModelLoadConfig),
    }

    #[derive(Deserialize, Debug)]
    pub struct LLMLoadConfig {
        pub context_length: u32,
        pub eval_batch_size: Option<u32>,
        pub flash_attention: Option<bool>,
        pub num_experts: Option<u32>,
        pub offload_kv_cache_to_gpu: Option<bool>,
    }

    #[derive(Deserialize, Debug)]
    pub struct EmbeddingModelLoadConfig {
        pub context_length: u32,
    }
}
