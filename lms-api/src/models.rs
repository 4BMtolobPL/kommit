pub mod download;
pub mod download_status;
pub mod load;
pub mod unload;

use crate::LmStudio;
use crate::error::ApiError;
use serde::Deserialize;
use tracing::{info, instrument};
use crate::types::{AllowedOptions, ModelFileFormat, ModelType};

impl LmStudio {
    #[instrument(skip(self))]
    pub async fn models(&self) -> Result<Vec<Model>, ApiError> {
        info!("List models");

        let url = format!("{}api/v1/models", self.url);

        let res = self.client.get(url).send().await?;

        if !res.status().is_success() {
            return Err(ApiError::Status(res.status()));
        }

        let models = res.json::<ModelsResponse>().await?;

        Ok(models.models)
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<Model>,
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
