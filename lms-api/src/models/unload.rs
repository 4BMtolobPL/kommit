use crate::LmStudio;
use crate::error::ApiError;
use serde::{Deserialize, Serialize};
use tracing::instrument;

impl LmStudio {
    #[instrument(skip(self))]
    pub async fn unload(&self, instance_id: &str) -> Result<String, ApiError> {
        let url = format!("{}api/v1/models/unload", self.url);

        let json = UnloadRequest::new(instance_id);
        let res = self.client.post(url).json(&json).send().await?;

        if !res.status().is_success() {
            return Err(ApiError::Status(res.status()));
        }

        let response = res.json::<UnloadResponse>().await?;
        Ok(response.instance_id)
    }
}

#[derive(Serialize)]
struct UnloadRequest {
    instance_id: String,
}

impl UnloadRequest {
    fn new(instance_id: &str) -> Self {
        Self {
            instance_id: instance_id.to_string(),
        }
    }
}

#[derive(Deserialize)]
struct UnloadResponse {
    instance_id: String,
}
