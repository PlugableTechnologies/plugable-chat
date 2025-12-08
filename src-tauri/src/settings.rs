use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;

// ============ Python Identifier Validation ============

/// Python reserved keywords that cannot be used as identifiers
const PYTHON_KEYWORDS: &[&str] = &[
    "False", "None", "True", "and", "as", "assert", "async", "await",
    "break", "class", "continue", "def", "del", "elif", "else", "except",
    "finally", "for", "from", "global", "if", "import", "in", "is",
    "lambda", "nonlocal", "not", "or", "pass", "raise", "return", "try",
    "while", "with", "yield",
];

/// Validate that a string is a valid Python identifier (module name).
/// 
/// Rules:
/// - Only lowercase letters, digits, and underscores
/// - Cannot start with a digit
/// - Cannot be a Python keyword
/// - Cannot be empty
pub fn validate_python_identifier(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Python identifier cannot be empty".to_string());
    }

    // Check first character (must be letter or underscore)
    let first_char = name.chars().next().unwrap();
    if !first_char.is_ascii_lowercase() && first_char != '_' {
        return Err(format!(
            "Python identifier must start with a lowercase letter or underscore, got '{}'",
            first_char
        ));
    }

    // Check all characters (must be lowercase letters, digits, or underscores)
    for (i, c) in name.chars().enumerate() {
        if !c.is_ascii_lowercase() && !c.is_ascii_digit() && c != '_' {
            return Err(format!(
                "Python identifier can only contain lowercase letters, digits, and underscores. \
                Invalid character '{}' at position {}",
                c, i
            ));
        }
    }

    // Check for Python keywords
    if PYTHON_KEYWORDS.contains(&name) {
        return Err(format!("'{}' is a Python reserved keyword", name));
    }

    Ok(())
}

/// Convert an arbitrary string to a valid Python identifier (snake_case).
///
/// Transformations:
/// - Convert to lowercase
/// - Replace spaces, hyphens, and other separators with underscores
/// - Remove invalid characters
/// - Prepend underscore if starts with digit
/// - Handle empty result
pub fn to_python_identifier(name: &str) -> String {
    let mut result = String::new();
    let mut last_was_underscore = false;

    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            // Convert to lowercase
            for lc in c.to_lowercase() {
                result.push(lc);
            }
            last_was_underscore = false;
        } else if c == ' ' || c == '-' || c == '_' || c == '.' {
            // Replace separators with underscore (avoiding duplicates)
            if !last_was_underscore && !result.is_empty() {
                result.push('_');
                last_was_underscore = true;
            }
        }
        // Skip other characters
    }

    // Remove trailing underscores
    while result.ends_with('_') {
        result.pop();
    }

    // Handle empty result
    if result.is_empty() {
        return "module".to_string();
    }

    // Prepend underscore if starts with digit
    if result.chars().next().unwrap().is_ascii_digit() {
        result = format!("_{}", result);
    }

    // Handle Python keywords by appending underscore
    if PYTHON_KEYWORDS.contains(&result.as_str()) {
        result.push('_');
    }

    result
}

// ============ Transport Types ============

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
    /// If true (default), tools from this server are deferred (hidden initially, discovered via tool_search)
    /// If false, tools are active (immediately visible to the model)
    #[serde(default = "default_defer_tools")]
    pub defer_tools: bool,
    /// Python module name for this server's tools (must be valid Python identifier).
    /// If not set, defaults to a sanitized version of the server id.
    /// Used for Python imports: `from {python_name} import tool_function`
    #[serde(default)]
    pub python_name: Option<String>,
}

fn default_defer_tools() -> bool {
    true
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
            defer_tools: true,
            python_name: None,
        }
    }

    /// Get the Python module name for this server.
    /// Returns the configured python_name, or derives one from the server id.
    pub fn get_python_name(&self) -> String {
        self.python_name
            .clone()
            .unwrap_or_else(|| to_python_identifier(&self.id))
    }

    /// Validate and set the Python module name.
    /// Returns an error if the name is not a valid Python identifier.
    pub fn set_python_name(&mut self, name: &str) -> Result<(), String> {
        validate_python_identifier(name)?;
        self.python_name = Some(name.to_string());
        Ok(())
    }
}

/// Ensure python_name is populated and sanitized from the display name.
pub fn enforce_python_name(config: &mut McpServerConfig) {
    let sanitized = to_python_identifier(&config.name);
    config.name = sanitized.clone();
    config.python_name = Some(sanitized);
}

/// Application settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    #[serde(default = "default_system_prompt")]
    pub system_prompt: String,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerConfig>,
    /// Optional system prompt snippets keyed by "{server_id}::{tool_name}".
    /// Use "builtin" as server_id for built-in tools.
    #[serde(default)]
    pub tool_system_prompts: HashMap<String, String>,
    /// Whether the python_execution built-in tool is enabled (disabled by default).
    /// When enabled, models can execute Python code in a sandboxed environment.
    /// Renamed from code_execution_enabled - alias preserved for backwards compatibility.
    #[serde(default, alias = "code_execution_enabled")]
    pub python_execution_enabled: bool,
}

fn default_system_prompt() -> String {
    r#"You are a helpful AI assistant. Be direct and concise in your responses. When you don't know something, say so rather than guessing."#.to_string()
}

/// Create the default MCP test server configuration
fn default_mcp_test_server() -> McpServerConfig {
    // Try to find the pre-built binary in common locations
    // Priority: target/release > cargo run
    let binary_path = std::env::current_dir().ok().and_then(|cwd| {
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

    let mut base = if let Some(path) = binary_path {
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "mcp_test_server_dev".to_string(),
            enabled: false, // Disabled by default
            transport: Transport::Stdio,
            command: Some(path),
            args: vec![],
            env: HashMap::new(),
            auto_approve_tools: true, // Auto-approve for dev testing
            defer_tools: true,        // Tools deferred by default (discovered via tool_search)
            python_name: None,
        }
    } else {
        // Fall back to cargo run if binary not found
        McpServerConfig {
            id: "mcp-test-server".to_string(),
            name: "mcp_test_server_dev".to_string(),
            enabled: false, // Disabled by default
            transport: Transport::Stdio,
            command: Some("cargo".to_string()),
            args: vec![
                "run".to_string(),
                "--manifest-path".to_string(),
                "mcp-test-server/Cargo.toml".to_string(),
                "--release".to_string(),
            ],
            env: HashMap::new(),
            auto_approve_tools: true, // Auto-approve for dev testing
            defer_tools: true,        // Tools deferred by default (discovered via tool_search)
            python_name: None,
        }
    };
    enforce_python_name(&mut base);
    base
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            system_prompt: default_system_prompt(),
            mcp_servers: vec![default_mcp_test_server()],
            tool_system_prompts: HashMap::new(),
            python_execution_enabled: false,
        }
    }
}

/// Ensure the default MCP test server exists in settings (for migration)
pub fn ensure_default_servers(settings: &mut AppSettings) {
    // Check if mcp-test-server already exists
    let has_test_server = settings
        .mcp_servers
        .iter()
        .any(|s| s.id == "mcp-test-server");

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
        Ok(contents) => match serde_json::from_str(&contents) {
            Ok(settings) => {
                println!("Settings loaded from {:?}", config_path);
                settings
            }
            Err(e) => {
                println!("Failed to parse settings: {}, using defaults", e);
                AppSettings::default()
            }
        },
        Err(e) => {
            println!(
                "No config file found at {:?}: {}, using defaults",
                config_path, e
            );
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
        assert!(settings
            .mcp_servers
            .iter()
            .any(|s| s.id == "mcp-test-server"));
        assert!(
            !settings
                .mcp_servers
                .iter()
                .find(|s| s.id == "mcp-test-server")
                .unwrap()
                .enabled
        );
        assert!(settings.tool_system_prompts.is_empty());
        // python_execution is disabled by default
        assert!(!settings.python_execution_enabled);
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
            defer_tools: true,
            python_name: Some("test_server".to_string()),
        });

        let json = serde_json::to_string(&settings).unwrap();
        let parsed: AppSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(settings.system_prompt, parsed.system_prompt);
        assert_eq!(settings.mcp_servers.len(), parsed.mcp_servers.len());
        assert_eq!(settings.mcp_servers[0].id, parsed.mcp_servers[0].id);
    }

    #[test]
    fn test_backwards_compat_code_execution_enabled() {
        // Test that old config files with "code_execution_enabled" still work
        let json =
            r#"{"system_prompt": "test", "mcp_servers": [], "code_execution_enabled": true}"#;
        let parsed: AppSettings = serde_json::from_str(json).unwrap();
        assert!(parsed.python_execution_enabled);
    }

    // ============ Python Identifier Validation Tests ============

    #[test]
    fn test_validate_python_identifier_valid() {
        assert!(validate_python_identifier("my_module").is_ok());
        assert!(validate_python_identifier("weather_api").is_ok());
        assert!(validate_python_identifier("mcp_test_server").is_ok());
        assert!(validate_python_identifier("_private").is_ok());
        assert!(validate_python_identifier("module123").is_ok());
        assert!(validate_python_identifier("a").is_ok());
    }

    #[test]
    fn test_validate_python_identifier_invalid() {
        // Empty
        assert!(validate_python_identifier("").is_err());

        // Starts with digit
        assert!(validate_python_identifier("123module").is_err());

        // Contains uppercase
        assert!(validate_python_identifier("MyModule").is_err());
        assert!(validate_python_identifier("myModule").is_err());

        // Contains invalid characters
        assert!(validate_python_identifier("my-module").is_err());
        assert!(validate_python_identifier("my.module").is_err());
        assert!(validate_python_identifier("my module").is_err());
        assert!(validate_python_identifier("my@module").is_err());

        // Python keywords
        assert!(validate_python_identifier("import").is_err());
        assert!(validate_python_identifier("class").is_err());
        assert!(validate_python_identifier("def").is_err());
        assert!(validate_python_identifier("None").is_err());
    }

    #[test]
    fn test_to_python_identifier() {
        // Basic conversion
        assert_eq!(to_python_identifier("My Module"), "my_module");
        assert_eq!(to_python_identifier("mcp-test-server"), "mcp_test_server");
        assert_eq!(to_python_identifier("Weather API"), "weather_api");

        // Handle leading digits
        assert_eq!(to_python_identifier("123abc"), "_123abc");

        // Handle special characters
        assert_eq!(to_python_identifier("my.module.name"), "my_module_name");
        assert_eq!(to_python_identifier("test@server#1"), "testserver1");

        // Handle multiple separators
        assert_eq!(to_python_identifier("my--module__name"), "my_module_name");

        // Handle empty/invalid input
        assert_eq!(to_python_identifier("@#$"), "module");
        assert_eq!(to_python_identifier(""), "module");

        // Handle Python keywords
        assert_eq!(to_python_identifier("import"), "import_");
        assert_eq!(to_python_identifier("class"), "class_");

        // Handle trailing separators
        assert_eq!(to_python_identifier("module_"), "module");
        assert_eq!(to_python_identifier("module--"), "module");
    }

    #[test]
    fn test_mcp_server_get_python_name() {
        // With explicit python_name
        let mut config = McpServerConfig::new("my-server".to_string(), "My Server".to_string());
        config.python_name = Some("custom_name".to_string());
        assert_eq!(config.get_python_name(), "custom_name");

        // Without explicit python_name (derived from id)
        let config2 = McpServerConfig::new("mcp-weather-api".to_string(), "Weather".to_string());
        assert_eq!(config2.get_python_name(), "mcp_weather_api");
    }

    #[test]
    fn test_mcp_server_set_python_name() {
        let mut config = McpServerConfig::new("test".to_string(), "Test".to_string());

        // Valid name
        assert!(config.set_python_name("my_module").is_ok());
        assert_eq!(config.python_name, Some("my_module".to_string()));

        // Invalid name
        assert!(config.set_python_name("My-Module").is_err());
        assert!(config.set_python_name("123abc").is_err());
        assert!(config.set_python_name("import").is_err());
    }

    #[test]
    fn test_enforce_python_name_sanitizes_name_and_python_name() {
        let mut config = McpServerConfig::new("server-1".to_string(), "Server Name 1".to_string());
        enforce_python_name(&mut config);
        assert_eq!(config.name, "server_name_1");
        assert_eq!(config.python_name.as_deref(), Some("server_name_1"));
    }
}
