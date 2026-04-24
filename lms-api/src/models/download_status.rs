use crate::LmStudio;
use crate::error::ApiError;
use crate::models::download_status::response::DownloadStatusResponse;
use tracing::{info, instrument};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models/download/status/{job_id}"))]
    pub async fn download_status(&self, job_id: &str) -> Result<DownloadStatusResponse, ApiError> {
        info!("Get the status of model downloads");

        let path = format!("api/v1/models/download/status/{job_id}");
        let url = self.endpoint(&path)?;
        let res = self.client.get(url).send().await?;

        self.handle_response(res).await
    }
}

pub mod response {
    use crate::types::DownloadStatus;
    use serde::Deserialize;

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
}
