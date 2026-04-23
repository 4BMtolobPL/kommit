use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ModelFileFormat {
    Gguf,
    Mlx,

    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ModelType {
    Llm,
    Embedding,

    #[serde(other)]
    Unknown,
}

impl Display for ModelType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ModelType::Llm => write!(f, "LLM"),
            ModelType::Embedding => write!(f, "Embedding"),
            ModelType::Unknown => write!(f, "Unknown"),
        }
    }
}

#[derive(Deserialize, Debug, Copy, Clone, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DownloadStatus {
    Downloading,
    Paused,
    Completed,
    Failed,
    AlreadyDownloaded,

    #[serde(other)]
    Unknown,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum LoadStatus {
    Loaded,

    #[serde(other)]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum AllowedOptions {
    Off,
    On,
    Low,
    Medium,
    High,

    #[serde(other)]
    Unknown,
}
