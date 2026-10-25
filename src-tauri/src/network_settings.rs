//! Optional network settings for machines behind a proxy or a firewall that blocks huggingface.co:
//! an HTTPS proxy, a Hugging Face mirror, and a folder holding the search model the user fetched
//! by other means.
//!
//! The proxy and the mirror reach the download code through the process environment (reqwest reads
//! `HTTPS_PROXY`, the model downloader reads `HF_ENDPOINT`), and the environment may only be changed
//! before other threads exist, so they are applied once at launch (`apply_launch_env`) and a change
//! takes effect at the next start. The model folder is read from the live settings each time the
//! search model loads, so "Retry" picks it up at once.

use serde::{Deserialize, Serialize};

pub const HTTPS_PROXY_ENV: &str = "HTTPS_PROXY";
pub const HF_ENDPOINT_ENV: &str = "HF_ENDPOINT";
/// Command-line / environment spelling of the offline folder, for scripts and the clean-host tests.
pub const OFFLINE_MODEL_FOLDER_ENV: &str = "PLUGABLE_OFFLINE_MODEL_FOLDER";

/// Saved under `network` in `config.json`. Every field is optional; an absent or blank value means
/// "use the defaults".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// e.g. `http://proxy.example.com:8080`. Used for every download the app makes.
    pub https_proxy: Option<String>,
    /// Base address of a Hugging Face mirror, e.g. `https://hf-mirror.example.com`.
    pub hf_endpoint: Option<String>,
    /// Folder containing the search model's files (`model.onnx`, `tokenizer.json`, ...).
    pub offline_model_folder: Option<String>,
}

fn clean(value: &Option<String>) -> Option<String> {
    value.as_ref().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

impl NetworkSettings {
    /// Trim whitespace, turn blanks into `None` and drop a trailing `/` from the mirror address.
    pub fn normalized(&self) -> NetworkSettings {
        NetworkSettings {
            https_proxy: clean(&self.https_proxy),
            hf_endpoint: clean(&self.hf_endpoint).map(|v| v.trim_end_matches('/').to_string()),
            offline_model_folder: clean(&self.offline_model_folder),
        }
    }

    /// Reject values that could never work, with a message the settings screen can show.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.normalized();
        if let Some(proxy) = &n.https_proxy {
            check_url("Proxy", proxy, &["http", "https", "socks5", "socks5h"])?;
        }
        if let Some(endpoint) = &n.hf_endpoint {
            check_url("Hugging Face mirror", endpoint, &["http", "https"])?;
        }
        if let Some(folder) = &n.offline_model_folder {
            if !std::path::Path::new(folder).is_dir() {
                return Err(format!("The model folder '{folder}' does not exist or is not a folder."));
            }
        }
        Ok(())
    }

    /// Environment variables to set at launch (only the fields that are filled in).
    pub fn env_assignments(&self) -> Vec<(&'static str, String)> {
        let n = self.normalized();
        let mut out = Vec::new();
        if let Some(proxy) = n.https_proxy {
            out.push((HTTPS_PROXY_ENV, proxy));
        }
        if let Some(endpoint) = n.hf_endpoint {
            out.push((HF_ENDPOINT_ENV, endpoint));
        }
        out
    }

    /// Fields in `over` that are filled in replace the ones here (command line beats saved settings).
    pub fn overlaid_with(&self, over: &NetworkSettings) -> NetworkSettings {
        let (base, over) = (self.normalized(), over.normalized());
        NetworkSettings {
            https_proxy: over.https_proxy.or(base.https_proxy),
            hf_endpoint: over.hf_endpoint.or(base.hf_endpoint),
            offline_model_folder: over.offline_model_folder.or(base.offline_model_folder),
        }
    }
}

fn check_url(what: &str, value: &str, schemes: &[&str]) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| format!("{what} address '{value}' is not a web address (expected something like http://host:port)."))?;
    if !schemes.contains(&url.scheme()) || url.host_str().is_none() {
        return Err(format!("{what} address '{value}' must start with {}.", schemes.iter().map(|s| format!("{s}://")).collect::<Vec<_>>().join(" or ")));
    }
    Ok(())
}

/// Read only the `network` section of the saved settings, synchronously, for use at launch before
/// the async runtime and the full settings loader exist. Missing file or section gives defaults.
pub fn load_saved_blocking(config_path: &std::path::Path) -> NetworkSettings {
    std::fs::read_to_string(config_path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get("network").cloned())
        .and_then(|v| serde_json::from_value::<NetworkSettings>(v).ok())
        .unwrap_or_default()
}

/// Set the proxy and mirror variables from `settings`. Must run before any other thread starts.
/// Returns the names set, for the log (never the values: a proxy address can carry credentials).
pub fn apply_launch_env(settings: &NetworkSettings) -> Vec<&'static str> {
    let assignments = settings.env_assignments();
    let names = assignments.iter().map(|(k, _)| *k).collect();
    for (key, value) in assignments {
        std::env::set_var(key, value);
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(proxy: &str, hf: &str, folder: &str) -> NetworkSettings {
        let o = |s: &str| if s.is_empty() { None } else { Some(s.to_string()) };
        NetworkSettings { https_proxy: o(proxy), hf_endpoint: o(hf), offline_model_folder: o(folder) }
    }

    #[test]
    fn blank_fields_mean_defaults_and_set_nothing() {
        let blank = ns("  ", "", "\t");
        assert_eq!(blank.normalized(), NetworkSettings::default());
        assert!(blank.env_assignments().is_empty());
        assert!(blank.validate().is_ok());
    }

    #[test]
    fn filled_fields_become_environment_variables_and_a_mirror_loses_its_trailing_slash() {
        let s = ns(" http://proxy.corp:8080 ", "https://hf-mirror.example.com/", "");
        assert_eq!(
            s.env_assignments(),
            vec![(HTTPS_PROXY_ENV, "http://proxy.corp:8080".to_string()), (HF_ENDPOINT_ENV, "https://hf-mirror.example.com".to_string())]
        );
    }

    #[test]
    fn unusable_addresses_are_refused_with_a_reason() {
        assert!(ns("proxy.corp:8080", "", "").validate().unwrap_err().contains("Proxy"));
        assert!(ns("ftp://proxy.corp", "", "").validate().is_err());
        assert!(ns("", "not a url", "").validate().unwrap_err().contains("mirror"));
        assert!(ns("socks5://127.0.0.1:1080", "http://mirror.local", "").validate().is_ok());
        assert!(ns("", "", "/definitely/not/here/xyz").validate().unwrap_err().contains("does not exist"));
        let dir = tempfile::tempdir().unwrap();
        assert!(ns("", "", dir.path().to_str().unwrap()).validate().is_ok());
    }

    #[test]
    fn command_line_values_beat_saved_values_field_by_field() {
        let saved = ns("http://saved:1", "https://saved-mirror", "/saved");
        let over = ns("", "https://cli-mirror", "");
        assert_eq!(saved.overlaid_with(&over), ns("http://saved:1", "https://cli-mirror", "/saved"));
    }

    #[test]
    fn settings_files_without_the_section_or_with_junk_load_as_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        assert_eq!(load_saved_blocking(&path), NetworkSettings::default());
        std::fs::write(&path, r#"{"system_prompt":"x"}"#).unwrap();
        assert_eq!(load_saved_blocking(&path), NetworkSettings::default());
        std::fs::write(&path, r#"{"network":{"https_proxy":"http://p:1","unknown":5}}"#).unwrap();
        assert_eq!(load_saved_blocking(&path), ns("http://p:1", "", ""));
        std::fs::write(&path, "not json").unwrap();
        assert_eq!(load_saved_blocking(&path), NetworkSettings::default());
    }

    #[test]
    fn app_settings_round_trip_keeps_the_network_section_and_old_files_still_load() {
        let mut s = crate::settings::AppSettings::default();
        s.network = ns("http://p:1", "", "/m");
        let back: crate::settings::AppSettings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.network, s.network);
        let mut v = serde_json::to_value(crate::settings::AppSettings::default()).unwrap();
        v.as_object_mut().unwrap().remove("network");
        let old: crate::settings::AppSettings = serde_json::from_value(v).unwrap();
        assert_eq!(old.network, NetworkSettings::default());
    }
}
