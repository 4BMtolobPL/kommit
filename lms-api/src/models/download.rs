use crate::LmStudio;
use crate::error::ApiError;
use crate::types::DownloadStatus;
use serde::{Deserialize, Serialize};
use tracing::{error, info, instrument};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models/download"))]
    pub async fn download(
        &self,
        model: &str,
        quantization: Option<&str>,
    ) -> Result<DownloadResponse, ApiError> {
        info!("Download LLMs and embedding models");

        let url = format!("{}api/v1/models/download", self.url);
        let request = DownloadRequest {
            model: model.to_string(),
            quantization: quantization.map(|x| x.to_string()),
        };
        let res = self.client.post(&url).json(&request).send().await?;

        let status = res.status();
        if !status.is_success() {
            error!(%url, "LM Studio request failed");

            let body = res.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        let response = res.json::<DownloadResponse>().await?;
        Ok(response)
    }
}
#[derive(Serialize)]
struct DownloadRequest {
    model: String,
    quantization: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct DownloadResponse {
    pub job_id: Option<String>,
    pub status: DownloadStatus,
    pub completed_at: Option<String>,
    pub total_size_bytes: Option<u64>,
    pub started_at: Option<String>,
}

impl DownloadResponse {
    pub fn job_id(&self) -> Option<String> {
        self.job_id.clone()
    }
}
