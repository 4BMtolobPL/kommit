use crate::LmStudio;
use crate::error::ApiError;
use crate::types::DownloadStatus;
use serde::Deserialize;
use tracing::{error, info, instrument};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models/download/status/{job_id}"))]
    pub async fn download_status(&self, job_id: &str) -> Result<DownloadStatusResponse, ApiError> {
        info!("Get the status of model downloads");
        
        let url = format!("{}api/v1/models/download/status/{job_id}", self.url);
        let res = self.client.get(&url).send().await?;

        let status = res.status();
        if !status.is_success() {
            error!(%url, "LM Studio request failed");

            let body = res.text().await.unwrap_or_default();
            return Err(ApiError::Status(status, body));
        }

        let response = res.json::<DownloadStatusResponse>().await?;
        Ok(response)
    }
}

#[derive(Deserialize, Debug)]
pub struct DownloadStatusResponse {
    pub job_id: String,
    pub status: DownloadStatus,
    pub bytes_per_second: Option<f64>,
    pub estimated_completion: Option<String>,
    pub completed_at: Option<String>,
    pub total_size_bytes: Option<u64>,
    pub downloaded_bytes: Option<u64>,
    pub started_at: Option<String>,
}

impl DownloadStatusResponse {
    pub fn job_id(&self) -> &str {
        &self.job_id
    }

    pub fn status(&self) -> DownloadStatus {
        self.status
    }
}
