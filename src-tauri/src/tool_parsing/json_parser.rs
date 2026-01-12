//! Pure JSON tool call parser.
//!
//! Parses tool calls from pure JSON without tags.

use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::{
    extract_tool_name_from_json, extract_tool_arguments_from_json, parse_combined_tool_name,
};
use super::json_fixer::parse_json_lenient;
use super::markdown_json_parser::parse_markdown_json_tool_calls;

/// Parse pure JSON object/array tool calls without tags.
pub fn parse_pure_json_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return calls;
    }

    if let Some(value) = parse_json_lenient(trimmed) {
        collect_tool_calls_from_json_value(&value, trimmed, &mut calls);
    }

    if calls.is_empty() {
        calls = parse_markdown_json_tool_calls(content);
    }

    calls
}

/// Collect tool calls from a JSON value (handles arrays and objects).
fn collect_tool_calls_from_json_value(value: &Value, raw: &str, calls: &mut Vec<ParsedToolCall>) {
    let entries: Vec<Value> = match value {
        Value::Array(arr) => arr.clone(),
        other => vec![other.clone()],
    };

    for entry in entries {
        // Handle {"tool": "...", "args": {...}}
        if let (Some(tool), Some(args)) = (
            entry.get("tool").and_then(|v| v.as_str()),
            entry.get("args"),
        ) {
            let (server, tool_name) = parse_combined_tool_name(tool);
            calls.push(ParsedToolCall {
                server,
                tool: tool_name,
                arguments: args.clone(),
                raw: raw.to_string(),
                id: None,
            });
            continue;
        }

        if let Some(name) = extract_tool_name_from_json(&entry) {
            let arguments = extract_tool_arguments_from_json(&entry);
            let (server, tool) = parse_combined_tool_name(&name);
            calls.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw: raw.to_string(),
                id: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pure_json_tool_call() {
        let content = r#"{"name": "test_tool", "arguments": {"x": 1}}"#;
        let calls = parse_pure_json_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "test_tool");
    }

    #[test]
    fn test_parse_pure_json_array() {
        let content = r#"[{"name": "tool1", "arguments": {}}, {"name": "tool2", "arguments": {}}]"#;
        let calls = parse_pure_json_tool_calls(content);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "tool1");
        assert_eq!(calls[1].tool, "tool2");
    }

    #[test]
    fn test_parse_pure_json_with_tool_args_format() {
        let content = r#"{"tool": "builtin___echo", "args": {"text": "hi"}}"#;
        let calls = parse_pure_json_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "builtin");
        assert_eq!(calls[0].tool, "echo");
    }
}
