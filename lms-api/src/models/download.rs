use crate::LmStudio;
use crate::error::ApiError;
use crate::models::download::request::DownloadRequest;
use crate::models::download::response::DownloadResponse;
use tracing::{error, info, instrument};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models/download"))]
    pub async fn download(&self, request: DownloadRequest) -> Result<DownloadResponse, ApiError> {
        info!("Download LLMs and embedding models");

        let url = format!("{}api/v1/models/download", self.url);
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

pub mod request {
    use derive_builder::Builder;
    use serde::Serialize;

    #[derive(Serialize, Debug, Builder)]
    #[builder(setter(into, strip_option))]
    pub struct DownloadRequest {
        model: String,
        #[builder(default)]
        quantization: Option<String>,
    }
}

pub mod response {
    use crate::types::DownloadStatus;
    use serde::Deserialize;

    #[derive(Deserialize, Debug)]
    pub struct DownloadResponse {
        pub job_id: Option<String>,
        pub status: DownloadStatus,
        pub completed_at: Option<String>,
        pub total_size_bytes: Option<u64>,
        pub started_at: Option<String>,
    }
}
