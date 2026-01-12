//! Common utilities and types for tool call parsing.

use serde_json::Value;

use crate::protocol::ParsedToolCall;

/// Extract tool name from parsed JSON, supporting multiple formats:
/// - `name` (standard)
/// - `tool_name` (GPT-OSS legacy)
/// - `function` (alternative)
/// - `tool` (when it's a string value, not an object)
/// - `action`, `command` (less common aliases)
/// - Nested paths: `tool.name`, `function.name`, `call.name`
pub fn extract_tool_name_from_json(parsed: &Value) -> Option<String> {
    // Direct field aliases (in order of preference)
    let name_fields = ["name", "tool_name", "function", "action", "command"];

    for field in name_fields {
        if let Some(name) = parsed.get(field).and_then(|v| v.as_str()) {
            // Validate it looks like a tool name
            if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                return Some(name.to_string());
            }
        }
    }

    // Check "tool" field - if it's a string, use it as the name
    if let Some(tool_val) = parsed.get("tool") {
        if let Some(name) = tool_val.as_str() {
            if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                return Some(name.to_string());
            }
        }
    }

    // Search nested paths
    let nested_paths = [
        ("tool", "name"),
        ("function", "name"),
        ("call", "name"),
        ("tool_call", "name"),
        ("function_call", "name"),
    ];

    for (outer, inner) in nested_paths {
        if let Some(outer_obj) = parsed.get(outer) {
            if let Some(name) = outer_obj.get(inner).and_then(|v| v.as_str()) {
                if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

/// Extract arguments from parsed JSON, supporting multiple formats:
/// - `arguments` (standard)
/// - `parameters` (Llama format)
/// - `tool_args` (GPT-OSS legacy)
pub fn extract_tool_arguments_from_json(parsed: &Value) -> Value {
    // Try "arguments" first (standard format)
    if let Some(args) = parsed.get("arguments") {
        return args.clone();
    }
    // Try "parameters" (Llama format)
    if let Some(args) = parsed.get("parameters") {
        return args.clone();
    }
    // Try "tool_args" (GPT-OSS legacy format)
    if let Some(args) = parsed.get("tool_args") {
        return args.clone();
    }
    // Default to empty object
    Value::Object(serde_json::Map::new())
}

/// Parse a combined "server___tool" name into (server, tool)
pub fn parse_combined_tool_name(combined: &str) -> (String, String) {
    let parts: Vec<&str> = combined.splitn(2, "___").collect();
    if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("unknown".to_string(), combined.to_string())
    }
}

/// Last-resort extraction: use regex to extract tool name and arguments directly.
/// This handles cases where JSON parsing fails completely.
pub fn extract_tool_call_by_regex(content: &str) -> Option<ParsedToolCall> {
    use regex::Regex;
    use super::json_fixer::parse_json_lenient;

    // Try to extract the name field
    let name_re = Regex::new(r#"["']?(name|tool_name)["']?\s*:\s*["']([^"']+)["']"#).ok()?;
    let name_cap = name_re.captures(content)?;
    let name = name_cap.get(2)?.as_str().to_string();

    if name.is_empty() || name.len() > 100 || name.contains('\n') {
        return None;
    }

    // Try to extract arguments - look for arguments/parameters/tool_args followed by an object
    let args_re =
        Regex::new(r#"["']?(arguments|parameters|tool_args)["']?\s*:\s*(\{[^{}]*\})"#).ok();
    let arguments = if let Some(args_re) = args_re {
        if let Some(args_cap) = args_re.captures(content) {
            if let Some(args_str) = args_cap.get(2) {
                parse_json_lenient(args_str.as_str()).unwrap_or(Value::Object(serde_json::Map::new()))
            } else {
                Value::Object(serde_json::Map::new())
            }
        } else {
            Value::Object(serde_json::Map::new())
        }
    } else {
        Value::Object(serde_json::Map::new())
    };

    let (server, tool) = parse_combined_tool_name(&name);

    Some(ParsedToolCall {
        server,
        tool,
        arguments,
        raw: content.to_string(),
        id: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_tool_name_from_json_nested_paths() {
        let input = json!({"tool": {"name": "nested_tool"}});
        let name = extract_tool_name_from_json(&input);
        assert_eq!(name, Some("nested_tool".to_string()));
    }

    #[test]
    fn test_extract_tool_name_from_json_function_alias() {
        let input = json!({"function": "my_function", "arguments": {}});
        let name = extract_tool_name_from_json(&input);
        assert_eq!(name, Some("my_function".to_string()));
    }

    #[test]
    fn test_extract_tool_name_from_json_tool_as_string() {
        let input = json!({"tool": "direct_tool_name", "args": {}});
        let name = extract_tool_name_from_json(&input);
        assert_eq!(name, Some("direct_tool_name".to_string()));
    }

    #[test]
    fn test_extract_tool_call_by_regex() {
        let content = r#"broken json here "name": "my_tool", "arguments": {"x": 1} more garbage"#;
        let call = extract_tool_call_by_regex(content);
        assert!(call.is_some(), "Should extract via regex fallback");
        let c = call.unwrap();
        assert_eq!(c.tool, "my_tool");
    }
}
