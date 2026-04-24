use crate::LmStudio;
use crate::error::ApiError;
use crate::models::unload::request::UnloadRequest;
use crate::models::unload::response::UnloadResponse;
use tracing::{info, instrument};

impl LmStudio {
    #[instrument(skip(self), fields(url = %self.url, endpoint = "/api/v1/models/unload"))]
    pub async fn unload(&self, instance_id: &str) -> Result<String, ApiError> {
        info!("Unload a loaded model from memory");

        let url = self.endpoint("api/v1/models/unload")?;
        let json = UnloadRequest::new(instance_id);
        let res = self.client.post(url).json(&json).send().await?;

        let response = self.handle_response::<UnloadResponse>(res).await?;
        Ok(response.instance_id)
    }
}

pub mod request {
    use serde::Serialize;

    #[derive(Serialize)]
    pub(super) struct UnloadRequest {
        instance_id: String,
    }

    impl UnloadRequest {
        pub(super) fn new(instance_id: &str) -> Self {
            Self {
                instance_id: instance_id.to_string(),
            }
        }
    }
}

mod response {
    use serde::Deserialize;

    #[derive(Deserialize)]
    pub(super) struct UnloadResponse {
        pub(super) instance_id: String,
    }
}
