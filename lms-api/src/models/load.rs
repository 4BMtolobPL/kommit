use crate::LmStudio;
use crate::error::ApiError;
use crate::models::types::{LoadStatus, ModelType};
use serde::{Deserialize, Serialize};
use tracing::instrument;

impl LmStudio {
    #[instrument(skip(self))]
    pub async fn load(&self, model: &str) -> Result<LoadResponse, ApiError> {
        let url = format!("{}api/v1/models/load", self.url);

        let json = LoadRequest::new(model);
        let res = self.client.post(url).json(&json).send().await?;

        if !res.status().is_success() {
            return Err(ApiError::Status(res.status()));
        }

        let response = res.json::<LoadResponse>().await?;
        Ok(response)
    }
}

#[derive(Serialize)]
struct LoadRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    context_length: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    eval_batch_size: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flash_attention: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_experts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offload_kv_cache_to_gpu: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    echo_load_config: Option<bool>,
}

impl LoadRequest {
    fn new(model: &str) -> Self {
        Self {
            model: model.to_string(),
            context_length: None,
            eval_batch_size: None,
            flash_attention: None,
            num_experts: None,
            offload_kv_cache_to_gpu: None,
            echo_load_config: None,
        }
    }
}

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
