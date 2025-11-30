use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

/// MCP Server transport type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Transport {
    Stdio,
    Sse { url: String },
}

impl Default for Transport {
    fn default() -> Self {
        Transport::Stdio
    }
}

/// Configuration for a single MCP server
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub transport: Transport,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub auto_approve_tools: bool,
    /// If true, tools from this server are deferred (hidden initially, discovered via tool_search)
    /// If false (default), tools are active (immediately visible to the model)
    #[serde(default)]
    pub defer_tools: bool,
}

impl McpServerConfig {
    pub fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            enabled: false,
            transport: Transport::Stdio,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            auto_approve_tools: false,
            defer_tools: false,
        }
    }
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
}

fn default_system_prompt() -> String {
    r#"You are a helpful AI assistant with tool-calling capabilities.

IMPORTANT: When a task can be helped by a tool, call the tool immediately. Don't explain what you would do - just do it.

For any math, calculations, or data processing: use code_execution. It gives exact results, not generated approximations."#.to_string()
}

/// Create the default MCP test server configuration
fn default_mcp_test_server() -> McpServerConfig {
    // Try to find the pre-built binary in common locations
    // Priority: target/release > cargo run
    let binary_path = std::env::current_dir()
        .ok()
        .and_then(|cwd| {
            let release_path = cwd.join("target/release/mcp-test-server");
            if release_path.exists() {
                Some(release_path.to_string_lossy().to_string())
            } else {
                let alt_path = cwd.join("mcp-test-server/target/release/mcp-test-server");
                if alt_path.exists() {
                    Some(alt_path.to_string_lossy().to_string())
                } else {
                    None
                }
            }
        });
    
    if let Some(path) = binary_path {
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "MCP Test Server (Dev)".to_string(),
            enabled: false,  // Disabled by default
            transport: Transport::Stdio,
            command: Some(path),
            args: vec![],
            env: HashMap::new(),
            auto_approve_tools: true,  // Auto-approve for dev testing
            defer_tools: false,  // Tools immediately visible
        }
    } else {
        // Fall back to cargo run if binary not found
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "MCP Test Server (Dev)".to_string(),
            enabled: false,  // Disabled by default
            transport: Transport::Stdio,
            command: Some("cargo".to_string()),
            args: vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                "mcp-test-server/Cargo.toml".to_string(),
                "--release".to_string(),
            ],
            env: HashMap::new(),
            auto_approve_tools: true,  // Auto-approve for dev testing
            defer_tools: false,  // Tools immediately visible
        }
    }
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: default_system_prompt(),
            mcp_servers: vec![default_mcp_test_server()],
        }
    }
}

/// Ensure the default MCP test server exists in settings (for migration)
pub fn ensure_default_servers(settings: &mut AppSettings) {
    // Check if mcp-test-server already exists
    let has_test_server = settings.mcp_servers.iter().any(|s| s.id == "mcp-test-server");
    
    if !has_test_server {
        println!("Adding default MCP test server to settings");
        settings.mcp_servers.insert(0, default_mcp_test_server());
    }
}

/// Get the path to the config file
fn get_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".plugable-chat").join("config.json")
}

/// Load settings from the config file
pub async fn load_settings() -> AppSettings {
    let config_path = get_config_path();
    
    let mut settings = match fs::read_to_string(&config_path).await {
        Ok(contents) => {
            match serde_json::from_str(&contents) {
                Ok(settings) => {
                    println!("Settings loaded from {:?}", config_path);
                    settings
                }
                Err(e) => {
                    println!("Failed to parse settings: {}, using defaults", e);
                    AppSettings::default()
                }
            }
        }
        Err(e) => {
            println!("No config file found at {:?}: {}, using defaults", config_path, e);
            AppSettings::default()
        }
    };
    
    // Ensure default servers exist (migration)
    ensure_default_servers(&mut settings);
    
    settings
}

/// Save settings to the config file
pub async fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let config_path = get_config_path();
    
    // Ensure the directory exists
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .await
            .map_err(|e| format!("Failed to create config directory: {}", e))?;
    }
    
    let contents = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Failed to serialize settings: {}", e))?;
    
    fs::write(&config_path, contents)
        .await
        .map_err(|e| format!("Failed to write config file: {}", e))?;
    
    println!("Settings saved to {:?}", config_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_settings() {
        let settings = AppSettings::default();
        assert!(!settings.system_prompt.is_empty());
        // Default settings include the mcp-test-server (disabled by default)
        assert!(settings.mcp_servers.iter().any(|s| s.id == "mcp-test-server"));
        assert!(!settings.mcp_servers.iter().find(|s| s.id == "mcp-test-server").unwrap().enabled);
    }

    #[test]
    fn test_serialize_deserialize() {
        let mut settings = AppSettings::default();
        settings.mcp_servers.push(McpServerConfig {
            id: "test-1".to_string(),
            name: "Test Server".to_string(),
            enabled: true,
            transport: Transport::Stdio,
            command: Some("node".to_string()),
            args: vec!["server.js".to_string()],
            env: HashMap::from([("DEBUG".to_string(), "true".to_string())]),
            auto_approve_tools: false,
            defer_tools: false,
        });

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();
        
        assert_eq!(settings.system_prompt, parsed.system_prompt);
        assert_eq!(settings.mcp_servers.len(), parsed.mcp_servers.len());
        assert_eq!(settings.mcp_servers[0].id, parsed.mcp_servers[0].id);
    }
}

