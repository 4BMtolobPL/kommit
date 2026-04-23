pub mod download;
pub mod download_status;
pub mod load;
pub mod unload;

use crate::LmStudio;
use crate::error::ApiError;
use crate::models::response::{Model, ModelsResponse};
use tracing::{error, info, instrument, warn};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models"))]
    pub async fn models(&self) -> Result<Vec<Model>, ApiError> {
        info!("Get a list of available models on your system, including both LLMs and embedding models.");

        let url = format!("{}api/v1/models", self.url);
        let res = self.client.get(&url).send().await?;

        let status = res.status();
        if !status.is_success() {
            error!(%url, "LM Studio request failed");

            let body = res.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        let models = res.json::<ModelsResponse>().await?;
        Ok(models.models)
    }
}

mod response {
    use crate::types::{AllowedOptions, ModelFileFormat, ModelType};
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct ModelsResponse {
        pub(super) models: Vec<Model>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Model {
        #[serde(rename = "type")]
        pub model_type: ModelType,
        pub publisher: String,
        pub key: String,
        pub display_name: String,
        pub architecture: Option<String>,
        pub quantization: Option<Quantization>,
        pub size_bytes: u64,
        pub params_string: Option<String>,
        pub loaded_instances: Vec<Instance>,
        pub max_context_length: u32,
        pub format: Option<ModelFileFormat>,
        pub capabilities: Option<Capability>,
        pub description: Option<String>,
        pub variants: Option<Vec<String>>,
        pub selected_variant: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Quantization {
        pub name: Option<String>,
        pub bits_per_weight: Option<i32>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Instance {
        pub id: String,
        pub config: Config,
    }

    #[derive(Deserialize, Debug)]
    pub struct Config {
        pub context_length: u32,
        pub eval_batch_size: Option<u32>,
        pub parallel: Option<u32>,
        pub flash_attention: Option<bool>,
        pub num_experts: Option<u32>,
        pub offload_kv_cache_to_gpu: Option<bool>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Capability {
        pub vision: bool,
        pub trained_for_tool_use: bool,
        pub reasoning: Option<Reasoning>,
    }

    #[derive(Deserialize, Debug)]
    pub struct Reasoning {
        pub allowed_options: Vec<AllowedOptions>,
        pub default: AllowedOptions,
    }
}
