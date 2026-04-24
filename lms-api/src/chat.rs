pub mod stream;

use crate::LmStudio;
use crate::chat::request::ChatRequest;
use crate::chat::response::ChatResponse;
use crate::error::ApiError;
use tracing::{info, instrument};

impl LmStudio {
    #[instrument(skip(self, request), fields(url = %self.url, endpoint = "/api/v1/chat", model = request.model))]
    pub async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, ApiError> {
        info!("Send a message to a model and receive a response. Supports MCP integration.");

        let url = self.endpoint("api/v1/chat")?;
        let res = self.client.post(url).json(&request).send().await?;

        self.handle_response(res).await
    }
}

pub mod request {
    use crate::types::AllowedOptions;
    use derive_builder::Builder;
    use serde::Serialize;
    use std::collections::HashMap;
    use std::str::FromStr;

    #[derive(Serialize, Builder)]
    #[builder(setter(into, strip_option))]
    pub struct ChatRequest {
        pub(super) model: String,
        input: Input,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        system_prompt: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        integrations: Option<Integration>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(setter(skip), default)]
        stream: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        temperature: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        top_p: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        top_k: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        min_p: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        repeat_penalty: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        max_output_tokens: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        reasoning: Option<AllowedOptions>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        context_length: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        store: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[builder(default)]
        previous_response_id: Option<String>,
    }

    impl ChatRequest {
        pub(super) fn with_stream(mut self, value: bool) -> Self {
            self.stream = Some(value);
            self
        }
    }

    #[derive(Serialize, Clone)]
    #[serde(untagged)]
    pub enum Input {
        InputText(String),
        InputObject(InputObject),
    }

    impl FromStr for Input {
        type Err = ();

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Self::from(s))
        }
    }

    impl From<&str> for Input {
        fn from(value: &str) -> Self {
            Self::InputText(value.to_string())
        }
    }

    #[derive(Serialize, Clone)]
    #[serde(tag = "type")]
    pub enum InputObject {
        Message { content: String },
        Image { data_url: String },
    }

    #[derive(Serialize, Clone)]
    #[serde(untagged)]
    enum Integration {
        PluginId(String),
        Integration(Tagged),
    }

    #[derive(Serialize, Clone)]
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
