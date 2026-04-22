use crate::LmStudio;
use crate::error::ApiError;
use crate::models::types::DownloadStatus;
use serde::Deserialize;

impl LmStudio {
    pub async fn download_status(&self, job_id: &str) -> Result<DownloadStatusResponse, ApiError> {
        let url = format!("{}api/v1/models/download/status/{job_id}", self.url);

        let res = self.client.get(url).send().await?;

        if !res.status().is_success() {
            return Err(ApiError::Status(res.status()));
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
