use crate::filter::FilterParams;
use crate::NmemError;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Default)]
pub struct NmemConfig {
    #[serde(default)]
    pub filter: FilterConfig,
    #[serde(default)]
    pub projects: HashMap<String, ProjectConfig>,
    #[serde(default)]
    pub encryption: EncryptionConfig,
}

#[derive(Debug, Deserialize, Default)]
pub struct FilterConfig {
    #[serde(default)]
    pub extra_patterns: Vec<String>,
    pub entropy_threshold: Option<f64>,
    pub entropy_min_length: Option<usize>,
    #[serde(default)]
    pub disable_entropy: bool,
}

#[derive(Debug, Deserialize, Default)]
pub struct ProjectConfig {
    #[serde(default)]
    pub sensitivity: Sensitivity,
}

#[derive(Debug, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    #[default]
    Default,
    Strict,
    Relaxed,
}

#[derive(Debug, Deserialize, Default)]
pub struct EncryptionConfig {
    pub key_file: Option<PathBuf>,
}

/// Load config from NMEM_CONFIG env var, ~/.nmem/config.toml, or defaults.
pub fn load_config() -> Result<NmemConfig, NmemError> {
    let path = config_path();
    match path {
        Some(p) if p.exists() => {
            let content = std::fs::read_to_string(&p)?;
            let config: NmemConfig = toml::from_str(&content)
                .map_err(|e| NmemError::Config(format!("{}: {e}", p.display())))?;
            validate_config(&config)?;
            Ok(config)
        }
        _ => Ok(NmemConfig::default()),
    }
}

fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("NMEM_CONFIG") {
        return Some(PathBuf::from(p));
    }
    let home = std::env::var("HOME").ok()?;
    Some(Path::new(&home).join(".nmem").join("config.toml"))
}

fn validate_config(config: &NmemConfig) -> Result<(), NmemError> {
    for (i, pat) in config.filter.extra_patterns.iter().enumerate() {
        Regex::new(pat).map_err(|e| {
            NmemError::Config(format!("extra_patterns[{i}] invalid regex: {e}"))
        })?;
    }
    Ok(())
}

/// Merge global config + project-specific settings into FilterParams.
pub fn resolve_filter_params(config: &NmemConfig, project: Option<&str>) -> FilterParams {
    let mut params = FilterParams {
        extra_patterns: config.filter.extra_patterns.clone(),
        entropy_threshold: config.filter.entropy_threshold.unwrap_or(4.0),
        entropy_min_length: config.filter.entropy_min_length.unwrap_or(20),
        entropy_enabled: !config.filter.disable_entropy,
    };

    // Apply project-level sensitivity (only if global hasn't explicitly disabled entropy)
    if let Some(proj) = project
        && let Some(pc) = config.projects.get(proj)
    {
        match pc.sensitivity {
            Sensitivity::Strict => {
                if config.filter.entropy_threshold.is_none() {
                    params.entropy_threshold = 3.5;
                }
                if config.filter.entropy_min_length.is_none() {
                    params.entropy_min_length = 16;
                }
            }
            Sensitivity::Relaxed => {
                if !config.filter.disable_entropy
                    && config.filter.entropy_threshold.is_none()
                {
                    params.entropy_enabled = false;
                }
            }
            Sensitivity::Default => {}
        }
    }

    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_when_no_file() {
        let config = NmemConfig::default();
        assert!(config.filter.extra_patterns.is_empty());
        assert_eq!(config.filter.entropy_threshold, None);
        assert!(!config.filter.disable_entropy);
        assert!(config.projects.is_empty());
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[filter]
extra_patterns = ["my-company-[A-Za-z0-9]{32}"]
entropy_threshold = 3.8
entropy_min_length = 16
disable_entropy = false

[projects.secret-app]
sensitivity = "strict"

[projects.open-source-tool]
sensitivity = "relaxed"

[encryption]
key_file = "/home/user/.nmem/custom-key"
"#;
        let config: NmemConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.filter.extra_patterns.len(), 1);
        assert_eq!(config.filter.entropy_threshold, Some(3.8));
        assert_eq!(config.filter.entropy_min_length, Some(16));
        assert_eq!(config.projects.len(), 2);
        assert_eq!(
            config.projects["secret-app"].sensitivity,
            Sensitivity::Strict
        );
        assert_eq!(
            config.projects["open-source-tool"].sensitivity,
            Sensitivity::Relaxed
        );
        assert_eq!(
            config.encryption.key_file,
            Some(PathBuf::from("/home/user/.nmem/custom-key"))
        );
    }

    #[test]
    fn invalid_regex_in_extra_patterns() {
        let config = NmemConfig {
            filter: FilterConfig {
                extra_patterns: vec!["[invalid".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let err = validate_config(&config);
        assert!(err.is_err());
    }

    #[test]
    fn resolve_params_strict() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
sensitivity = "strict"
"#,
        )
        .unwrap();
        let params = resolve_filter_params(&config, Some("myproj"));
        assert_eq!(params.entropy_threshold, 3.5);
        assert_eq!(params.entropy_min_length, 16);
        assert!(params.entropy_enabled);
    }

    #[test]
    fn resolve_params_relaxed_disables_entropy() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
sensitivity = "relaxed"
"#,
        )
        .unwrap();
        let params = resolve_filter_params(&config, Some("myproj"));
        assert!(!params.entropy_enabled);
    }

    #[test]
    fn global_override_trumps_sensitivity() {
        let config: NmemConfig = toml::from_str(
            r#"
[filter]
entropy_threshold = 4.5

[projects.myproj]
sensitivity = "strict"
"#,
        )
        .unwrap();
        let params = resolve_filter_params(&config, Some("myproj"));
        // Global threshold should prevail over strict's default
        assert_eq!(params.entropy_threshold, 4.5);
    }

    #[test]
    fn extra_patterns_applied() {
        let config: NmemConfig = toml::from_str(
            r#"
[filter]
extra_patterns = ["MYCO-[A-Za-z0-9]{32}"]
"#,
        )
        .unwrap();
        let params = resolve_filter_params(&config, None);
        assert_eq!(params.extra_patterns.len(), 1);
    }
}
