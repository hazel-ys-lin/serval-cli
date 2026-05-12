//! User-global config at `~/.serval/config.toml`.
//!
//! Single-user, single-file. Holds named environments
//! (each with a `base_url`) plus an optional `default_env`.
//!
//! ```toml
//! default_env = "local"
//!
//! [envs.local]
//! base_url = "http://localhost:3000"
//!
//! [envs.staging]
//! base_url = "https://staging.example.com"
//! ```
//!
//! Resolution order for `serval run`'s effective base URL:
//! 1. explicit `--base-url` flag (always wins)
//! 2. `--env <name>` lookup in `config.envs`
//! 3. `default_env` lookup in `config.envs`
//! 4. error
//!
//! Override the config file path via `$SERVAL_CONFIG_FILE` (mostly
//! useful in tests and ad-hoc shells).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_env: Option<String>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub envs: BTreeMap<String, EnvConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvConfig {
    pub base_url: String,
}

impl Config {
    /// Look up a specific env, or — when `name` is `None` — the
    /// configured `default_env`. Returns `(resolved_name, env)`.
    pub fn resolve_env(&self, name: Option<&str>) -> Option<(&str, &EnvConfig)> {
        let target = name.or(self.default_env.as_deref())?;
        self.envs
            .get_key_value(target)
            .map(|(k, v)| (k.as_str(), v))
    }
}

/// Compute the user's config file path. Resolution:
/// 1. `$SERVAL_CONFIG_FILE` if set
/// 2. `$HOME/.serval/config.toml` otherwise
pub fn default_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("SERVAL_CONFIG_FILE") {
        return Ok(PathBuf::from(p));
    }
    let home = std::env::var("HOME")
        .map_err(|_| Error::System("$HOME is not set; cannot locate ~/.serval/".into()))?;
    Ok(PathBuf::from(home).join(".serval").join("config.toml"))
}

/// Load the config at `path`. Missing file is fine and yields the
/// default (empty) config — first-run UX.
pub fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        return Ok(Config::default());
    }
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::System(format!("read config {}: {e}", path.display())))?;
    toml::from_str(&content).map_err(|e| Error::Spec(format!("config {}: {e}", path.display())))
}

/// Persist `config` at `path`, creating parent directories as needed.
pub fn save(path: &Path, config: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::System(format!("create config dir {}: {e}", parent.display())))?;
    }
    let content = toml::to_string_pretty(config)
        .map_err(|e| Error::System(format!("serialize config: {e}")))?;
    std::fs::write(path, content)
        .map_err(|e| Error::System(format!("write config {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_roundtrips() {
        let cfg = Config::default();
        let s = toml::to_string_pretty(&cfg).unwrap();
        let parsed: Config = toml::from_str(&s).unwrap();
        assert!(parsed.default_env.is_none());
        assert!(parsed.envs.is_empty());
    }

    #[test]
    fn missing_file_yields_default() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.toml");
        let cfg = load(&missing).unwrap();
        assert!(cfg.envs.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sub").join("config.toml");

        let mut envs = BTreeMap::new();
        envs.insert(
            "local".to_string(),
            EnvConfig {
                base_url: "http://localhost:3000".into(),
            },
        );
        let cfg = Config {
            default_env: Some("local".into()),
            envs,
        };
        save(&path, &cfg).unwrap();

        let reloaded = load(&path).unwrap();
        assert_eq!(reloaded.default_env.as_deref(), Some("local"));
        assert_eq!(
            reloaded.envs.get("local").unwrap().base_url,
            "http://localhost:3000"
        );
    }

    #[test]
    fn resolve_env_falls_back_to_default() {
        let mut envs = BTreeMap::new();
        envs.insert(
            "local".to_string(),
            EnvConfig {
                base_url: "http://localhost:3000".into(),
            },
        );
        let cfg = Config {
            default_env: Some("local".into()),
            envs,
        };

        let (name, env) = cfg.resolve_env(None).expect("default_env should resolve");
        assert_eq!(name, "local");
        assert_eq!(env.base_url, "http://localhost:3000");
    }

    #[test]
    fn resolve_env_returns_none_when_unknown() {
        let cfg = Config::default();
        assert!(cfg.resolve_env(Some("nope")).is_none());
        assert!(cfg.resolve_env(None).is_none());
    }
}
