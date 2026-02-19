use crate::s5_filter::FilterParams;
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
    #[serde(default)]
    pub retention: RetentionConfig,
    #[serde(default)]
    pub metrics: crate::metrics::MetricsConfig,
    #[serde(default)]
    pub summarization: SummarizationConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SummarizationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_summarization_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_summarization_model")]
    pub model: String,
    #[serde(default = "default_summarization_timeout")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub fallback_endpoint: Option<String>,
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_summarization_endpoint(),
            model: default_summarization_model(),
            timeout_secs: default_summarization_timeout(),
            fallback_endpoint: None,
        }
    }
}

fn default_summarization_endpoint() -> String {
    "http://localhost:1234/v1/chat/completions".into()
}

fn default_summarization_model() -> String {
    "ibm/granite-4-h-tiny".into()
}

fn default_summarization_timeout() -> u64 {
    30
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
    /// Max local-project observations in context injection (default: 20 normal, 30 recovery).
    pub context_local_limit: Option<u32>,
    /// Max cross-project observations in context injection (default: 10 normal, 15 recovery).
    pub context_cross_limit: Option<u32>,
    /// Suppress cross-project observations in context injection (default: false).
    /// Takes precedence over `context_cross_limit` when true.
    #[serde(default)]
    pub suppress_cross_project: bool,
    /// Episode window in hours for context injection (default: 48).
    /// Episodes within this window replace session summaries.
    pub context_episode_window_hours: Option<u32>,
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

#[derive(Debug, Deserialize)]
pub struct RetentionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_retention_days")]
    pub days: HashMap<String, u32>,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            days: default_retention_days(),
        }
    }
}

fn default_retention_days() -> HashMap<String, u32> {
    HashMap::from([
        ("user_prompt".into(), 730),
        ("command_error".into(), 730),
        ("file_write".into(), 365),
        ("file_edit".into(), 365),
        ("session_start".into(), 365),
        ("session_end".into(), 365),
        ("file_read".into(), 90),
        ("search".into(), 90),
        ("mcp_call".into(), 90),
        ("command".into(), 180),
    ])
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

/// Resolve context injection limits from config.
/// Explicit per-project limits are used as-is (recovery mode does NOT multiply them).
/// Without explicit config, defaults are 20/10 (normal) or 30/15 (recovery).
pub fn resolve_context_limits(config: &NmemConfig, project: &str, is_recovery: bool) -> (i64, i64) {
    let pc = config.projects.get(project);
    let local = pc
        .and_then(|p| p.context_local_limit)
        .map(|v| v as i64)
        .unwrap_or(if is_recovery { 30 } else { 20 });
    let cross = if pc.is_some_and(|p| p.suppress_cross_project) {
        0
    } else {
        pc.and_then(|p| p.context_cross_limit)
            .map(|v| v as i64)
            .unwrap_or(if is_recovery { 15 } else { 10 })
    };
    (local, cross)
}

/// Resolve episode window in seconds from config.
/// Project override takes precedence, otherwise default 48 hours.
pub fn resolve_episode_window(config: &NmemConfig, project: &str) -> i64 {
    let hours = config
        .projects
        .get(project)
        .and_then(|p| p.context_episode_window_hours)
        .unwrap_or(48);
    hours as i64 * 3600
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
    fn default_retention_config() {
        let config = NmemConfig::default();
        assert!(!config.retention.enabled);
        assert_eq!(config.retention.days["file_read"], 90);
        assert_eq!(config.retention.days["command"], 180);
        assert_eq!(config.retention.days["file_edit"], 365);
        assert_eq!(config.retention.days["user_prompt"], 730);
    }

    #[test]
    fn parse_retention_config() {
        let toml_str = r#"
[retention]
enabled = true

[retention.days]
file_read = 30
command = 60
"#;
        let config: NmemConfig = toml::from_str(toml_str).unwrap();
        assert!(config.retention.enabled);
        assert_eq!(config.retention.days["file_read"], 30);
        assert_eq!(config.retention.days["command"], 60);
        // Custom days map replaces defaults entirely
        assert!(!config.retention.days.contains_key("user_prompt"));
    }

    #[test]
    fn context_limits_defaults_normal() {
        let config = NmemConfig::default();
        let (local, cross) = resolve_context_limits(&config, "unknown", false);
        assert_eq!(local, 20);
        assert_eq!(cross, 10);
    }

    #[test]
    fn context_limits_defaults_recovery() {
        let config = NmemConfig::default();
        let (local, cross) = resolve_context_limits(&config, "unknown", true);
        assert_eq!(local, 30);
        assert_eq!(cross, 15);
    }

    #[test]
    fn context_limits_custom_ignores_recovery() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
context_local_limit = 50
context_cross_limit = 5
"#,
        )
        .unwrap();
        // Normal
        let (local, cross) = resolve_context_limits(&config, "myproj", false);
        assert_eq!(local, 50);
        assert_eq!(cross, 5);
        // Recovery — same values, NOT multiplied
        let (local, cross) = resolve_context_limits(&config, "myproj", true);
        assert_eq!(local, 50);
        assert_eq!(cross, 5);
    }

    #[test]
    fn context_limits_partial_override() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
context_local_limit = 40
"#,
        )
        .unwrap();
        // local is explicit, cross falls back to default
        let (local, cross) = resolve_context_limits(&config, "myproj", false);
        assert_eq!(local, 40);
        assert_eq!(cross, 10);
        // recovery: local still explicit, cross gets recovery default
        let (local, cross) = resolve_context_limits(&config, "myproj", true);
        assert_eq!(local, 40);
        assert_eq!(cross, 15);
    }

    #[test]
    fn context_limits_unknown_project_uses_defaults() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.other]
context_local_limit = 99
"#,
        )
        .unwrap();
        let (local, cross) = resolve_context_limits(&config, "unknown", false);
        assert_eq!(local, 20);
        assert_eq!(cross, 10);
    }

    #[test]
    fn suppress_cross_project_overrides_limits() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
suppress_cross_project = true
context_cross_limit = 5
"#,
        )
        .unwrap();
        let (_, cross) = resolve_context_limits(&config, "myproj", false);
        assert_eq!(cross, 0, "suppress_cross_project should override context_cross_limit");
        let (_, cross) = resolve_context_limits(&config, "myproj", true);
        assert_eq!(cross, 0, "suppress_cross_project should override recovery defaults too");
    }

    #[test]
    fn suppress_cross_project_default_false() {
        let config: NmemConfig = toml::from_str(
            r#"
[projects.myproj]
"#,
        )
        .unwrap();
        let (_, cross) = resolve_context_limits(&config, "myproj", false);
        assert_eq!(cross, 10, "default config should not suppress cross-project");
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
