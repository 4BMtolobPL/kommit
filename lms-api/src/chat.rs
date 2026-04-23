use tracing::info;
use crate::chat::request::{ChatRequest, Input};
use crate::chat::response::ChatResponse;
use crate::error::ApiError;
use crate::LmStudio;

impl LmStudio {
    pub async fn chat(&self, model: &str, input: &str) -> Result<ChatResponse, ApiError> {
        info!("Send a message to model");

        let url = format!("{}api/v1/chat", self.url);

        let json = ChatRequest::new(model, input);

        let res = self.client.post(url).json(&json).send().await?;

        if !res.status().is_success() {
            return Err(ApiError::Status(res.status()));
        }

        let response = res.json::<ChatResponse>().await?;

        Ok(response)
    }
}

mod request {
    use crate::types::AllowedOptions;
    use serde::Serialize;
    use std::collections::HashMap;

    #[derive(Serialize)]
    pub struct ChatRequest {
        model: String,
        input: Input,
        system_prompt: Option<String>,
        integrations: Option<Integration>,
        stream: Option<bool>,
        temperature: Option<f64>,
        top_p: Option<f64>,
        top_k: Option<u32>,
        min_p: Option<f64>,
        repeat_penalty: Option<f64>,
        max_output_tokens: Option<u32>,
        reasoning: Option<AllowedOptions>,
        context_length: Option<u32>,
        store: Option<bool>,
        previous_response_id: Option<String>,
    }

    impl ChatRequest {
        pub fn new(model: &str, input: &str) -> Self {
            Self {
                model: model.to_string(),
                input: Input::InputText(input.to_string()),
                system_prompt: None,
                integrations: None,
                stream: None,
                temperature: None,
                top_p: None,
                top_k: None,
                min_p: None,
                repeat_penalty: None,
                max_output_tokens: None,
                reasoning: None,
                context_length: None,
                store: None,
                previous_response_id: None,
            }
        }
    }

    #[derive(Serialize)]
    pub(super) enum Input {
        InputText(String),
        InputObject(InputObject),
    }

    #[derive(Serialize)]
    #[serde(tag = "type")]
    enum InputObject {
        Message {
            content: String,
        },
        Image {
            data_url: String,
        }
    }


    #[derive(Serialize)]
    enum Integration {
        PluginId(String),
        Integration(Tagged),
    }

    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Tagged {
        Plugin {
            id: String,
            allowed_tools: Option<Vec<String>>,
        },
        EphemeralMcp {
            server_label: String,
            server_url: String,
            allowed_tools: Option<Vec<String>>,
            headers: Option<HashMap<String, String>>,
        }
    }
}

mod response {
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Deserialize)]
    pub struct ChatResponse {
        model_instance_id: String,
        output: Vec<Output>,
        stats: Stats,
        response_id: Option<String>,
    }

    #[derive(Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Output {
        Message {
            content: String,
        },
        ToolCall {
            tool: String,
            arguments: HashMap<String, String>,
            output: String,
            provider_info: ProviderInfo,
        },
        Reasoning {
            content: String,
        },
        InvalidToolCall {
            reason: String,
            metadata: Metadata
        },
    }

    #[derive(Deserialize)]
    #[serde(tag = "type")]
    enum ProviderInfo {
        Plugin {
            plugin_id: String,
        },
        EphemeralMcp {
            server_label: String,
        }
    }

    #[derive(Deserialize)]
    #[serde(tag = "type")]
    enum Metadata {
        InvalidName {
            tool_name: String,
        },
        InvalidArguments {
            tool_name: String,
            arguments: HashMap<String, String>,
            provider_info: ProviderInfo
        }
    }

    #[derive(Deserialize)]
    struct Stats {
        input_tokens : f64,
        total_output_tokens : f64,
        reasoning_output_tokens : f64,
        tokens_per_second : f64,
        time_to_first_token_seconds : f64,
        model_load_time_seconds : Option<f64>
    }
}