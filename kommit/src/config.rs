use crate::prompt::ResponseLang;
use crate::provider::{LlmProvider, ThinkType};
use anyhow::Context;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    pub(crate) model: Option<String>,
    pub(crate) lang: Option<ResponseLang>,
    pub(crate) provider: Option<LlmProvider>,
    pub(crate) stream: Option<bool>,
    pub(crate) think: Option<ThinkType>,
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
    /// Context length in tokens the model is loaded with.
    pub(crate) context_length: Option<u32>,
    pub(crate) exclude_patterns: Option<Vec<String>>,
}

impl Config {
    /// Loads the config file, or the defaults if there is no config file.
    ///
    /// A file that cannot be read or parsed is an error rather than the defaults,
    /// so that `config set` never overwrites settings it failed to load.
    pub(crate) fn load() -> anyhow::Result<Self> {
        match Self::get_config_path() {
            Some(path) => Self::load_from(&path),
            // Without a home directory there is no config file to read.
            None => Ok(Self::default()),
        }
    }

    fn load_from(path: &Path) -> anyhow::Result<Self> {
        debug!(?path, "Checking for config file");
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("Failed to read config file {}", path.display()));
            }
        };
        let config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file {}", path.display()))?;
        debug!(?config, "Loaded config from file");
        Ok(config)
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

    pub(crate) fn get_exclude_patterns(&self) -> Vec<String> {
        self.exclude_patterns.clone().unwrap_or_else(|| {
            vec![
                "*.lock".to_string(),
                "package-lock.json".to_string(),
                "pnpm-lock.yaml".to_string(),
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    fn load_str(content: &str) -> (tempfile::TempDir, PathBuf, anyhow::Result<Config>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, content).unwrap();
        let result = Config::load_from(&path);
        (dir, path, result)
    }

    #[test]
    fn missing_file_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::load_from(&dir.path().join("config.toml")).unwrap();
        assert!(config.model.is_none());
        assert!(config.port.is_none());
        assert!(config.exclude_patterns.is_none());
    }

    #[test]
    fn valid_file_is_loaded() {
        let (_dir, _path, result) = load_str(
            "model = \"llama\"\nport = 1234\ncontext_length = 4096\nexclude_patterns = [\"*.min.js\"]\n",
        );
        let config = result.unwrap();
        assert_eq!(config.context_length, Some(4096));
        assert_eq!(config.model.as_deref(), Some("llama"));
        assert_eq!(config.port, Some(1234));
        assert_eq!(config.exclude_patterns, Some(vec!["*.min.js".to_string()]));
    }

    #[test]
    fn invalid_value_is_an_error_naming_the_file() {
        let (_dir, path, result) = load_str("provider = \"invalid\"\n");
        let message = format!("{:#}", result.unwrap_err());
        assert!(
            message.contains(&path.display().to_string()),
            "message was {message}"
        );
        assert!(message.contains("invalid"), "message was {message}");
    }

    #[test]
    fn unknown_key_is_an_error() {
        let (_dir, _path, result) = load_str("modle = \"x\"\n");
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains("modle"), "message was {message}");
    }

    /// Writes `config` the way `save` does and loads it back.
    fn round_trip(config: &Config) -> (String, Config) {
        let content = toml::to_string_pretty(config).unwrap();
        let (_dir, _path, result) = load_str(&content);
        (content, result.unwrap())
    }

    /// The name of `value` on the command line, which the config file must use too.
    fn cli_name(value: &impl ValueEnum) -> String {
        value.to_possible_value().unwrap().get_name().to_owned()
    }

    #[test]
    fn every_provider_is_saved_by_its_cli_name_and_loaded_back() {
        for provider in LlmProvider::value_variants() {
            let config = Config {
                provider: Some(provider.clone()),
                ..Config::default()
            };
            let (content, loaded) = round_trip(&config);
            assert_eq!(
                content.trim(),
                format!("provider = \"{}\"", cli_name(provider))
            );
            assert_eq!(
                loaded.provider.as_ref().map(cli_name),
                Some(cli_name(provider))
            );
        }
    }

    #[test]
    fn every_think_type_is_saved_by_its_cli_name_and_loaded_back() {
        for think in ThinkType::value_variants() {
            let config = Config {
                think: Some(think.clone()),
                ..Config::default()
            };
            let (content, loaded) = round_trip(&config);
            assert_eq!(content.trim(), format!("think = \"{think}\""));
            assert_eq!(loaded.think.as_ref().map(cli_name), Some(cli_name(think)));
        }
    }

    #[test]
    fn every_lang_is_saved_by_its_cli_name_and_loaded_back() {
        for lang in ResponseLang::value_variants() {
            let config = Config {
                lang: Some(lang.clone()),
                ..Config::default()
            };
            let (content, loaded) = round_trip(&config);
            assert_eq!(content.trim(), format!("lang = \"{lang}\""));
            assert_eq!(loaded.lang.as_ref().map(cli_name), Some(cli_name(lang)));
        }
    }

    #[test]
    fn lmstudio_without_a_dash_is_loaded_as_lm_studio() {
        let (_dir, _path, result) = load_str("provider = \"lmstudio\"\n");
        assert!(matches!(
            result.unwrap().provider,
            Some(LlmProvider::LmStudio)
        ));
    }

    #[test]
    fn a_full_config_survives_a_round_trip() {
        let config = Config {
            model: Some("llama".to_string()),
            lang: Some(ResponseLang::Ko),
            provider: Some(LlmProvider::LmStudio),
            stream: Some(true),
            think: Some(ThinkType::High),
            host: Some("http://example.com:8080".to_string()),
            port: Some(8080),
            context_length: Some(8192),
            exclude_patterns: Some(vec!["*.min.js".to_string(), "dist/*".to_string()]),
        };
        let (content, loaded) = round_trip(&config);
        assert_eq!(toml::to_string_pretty(&loaded).unwrap(), content);
    }

    #[test]
    fn unreadable_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let message = format!("{:#}", Config::load_from(dir.path()).unwrap_err());
        assert!(
            message.contains("Failed to read config file"),
            "message was {message}"
        );
    }
}
