use crate::LmStudio;
use crate::chat::request::ChatRequest;
use crate::chat::response::ChatResponse;
use crate::error::ApiError;
use tracing::info;

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
        #[serde(skip_serializing_if = "Option::is_none")]
        system_prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        integrations: Option<Integration>,
        #[serde(skip_serializing_if = "Option::is_none")]
        stream: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        temperature: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        top_p: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        top_k: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        min_p: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        repeat_penalty: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<AllowedOptions>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_length: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        store: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
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
    #[serde(untagged)]
    pub(super) enum Input {
        InputText(String),
        InputObject(InputObject),
    }

    #[derive(Serialize)]
    #[serde(tag = "type")]
    pub(super) enum InputObject {
        Message { content: String },
        Image { data_url: String },
    }

    #[derive(Serialize)]
    #[serde(untagged)]
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
        },
    }
}

pub mod response {
    use serde::Deserialize;
    use std::collections::HashMap;

    #[derive(Deserialize, Debug)]
    pub struct ChatResponse {
        pub model_instance_id: String,
        pub output: Vec<Output>,
        pub stats: Stats,
        pub response_id: Option<String>,
    }

    #[derive(Deserialize, Debug)]
    #[serde(tag = "type")]
    pub enum ProviderInfo {
        Plugin { plugin_id: String },
        EphemeralMcp { server_label: String },
    }

    #[derive(Deserialize, Debug)]
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum Output {
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
            metadata: Metadata,
        },
    }

    #[derive(Deserialize, Debug)]
    #[serde(tag = "type")]
    pub enum Metadata {
        InvalidName {
            tool_name: String,
        },
        InvalidArguments {
            tool_name: String,
            arguments: HashMap<String, String>,
            provider_info: ProviderInfo,
        },
    }

    #[derive(Deserialize, Debug)]
    pub struct Stats {
        pub input_tokens: f64,
        pub total_output_tokens: f64,
        pub reasoning_output_tokens: f64,
        pub tokens_per_second: f64,
        pub time_to_first_token_seconds: f64,
        pub model_load_time_seconds: Option<f64>,
    }
}
