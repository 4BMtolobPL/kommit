use crate::prompt::ResponseLang;
use crate::provider::{LlmProvider, ThinkType};
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tracing::{debug, warn};

#[derive(Debug, Deserialize, Serialize, Default)]
pub(crate) struct Config {
    pub(crate) model: Option<String>,
    pub(crate) lang: Option<ResponseLang>,
    pub(crate) provider: Option<LlmProvider>,
    pub(crate) stream: Option<bool>,
    pub(crate) think: Option<ThinkType>,
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
}

impl Config {
    pub(crate) fn load() -> Self {
        let config_path = Self::get_config_path();

        if let Some(path) = config_path {
            debug!(?path, "Checking for config file");
            if path.exists() {
                match fs::read_to_string(&path) {
                    Ok(content) => match toml::from_str(&content) {
                        Ok(config) => {
                            debug!(?config, "Loaded config from file");
                            return config;
                        }
                        Err(e) => {
                            warn!(?e, "Failed to parse config file");
                        }
                    },
                    Err(e) => {
                        warn!(?e, "Failed to read config file");
                    }
                }
            }
        }

        Config::default()
    }

    pub(crate) fn save(&self) -> anyhow::Result<()> {
        let path = Self::get_config_path().context("Failed to get config path")?;

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).context("Failed to create config directory")?;
        }

        let content = toml::to_string_pretty(self).context("Failed to serialize config")?;
        fs::write(path, content).context("Failed to write config file")?;

        Ok(())
    }

    pub(crate) fn get_config_path() -> Option<PathBuf> {
        ProjectDirs::from("", "", "kommit")
            .map(|proj_dirs| proj_dirs.config_dir().join("config.toml"))
    }

    pub(crate) fn get_config_dir() -> Option<PathBuf> {
        ProjectDirs::from("", "", "kommit").map(|proj_dirs| proj_dirs.config_dir().to_path_buf())
    }
}
