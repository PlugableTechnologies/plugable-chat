//! Granite format tool call parser.
//!
//! Parses tool calls in the format: `<function_call>...</function_call>`
//! Used by Granite and Gemma models.

use regex::Regex;
use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::{
    extract_tool_name_from_json, extract_tool_arguments_from_json, parse_combined_tool_name,
};
use super::json_fixer::repair_malformed_json;
use super::hermes_parser::parse_hermes_tool_calls;

/// Parse Granite <function_call> format (also used by Gemma)
/// Handles JSON inside <function_call> tags with flexible field names
pub fn parse_granite_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Granite/Gemma uses <function_call> tags
    let re = Regex::new(r"(?s)<function_call>\s*(.*?)\s*</function_call>").ok();

    if let Some(re) = re {
        for cap in re.captures_iter(content) {
            if let Some(inner) = cap.get(1) {
                let call_content = inner.as_str().trim();

                // Try parsing as JSON first
                let fixed_json = repair_malformed_json(call_content);
                if let Ok(parsed) = serde_json::from_str::<Value>(&fixed_json) {
                    // Use helper functions for flexible field name extraction
                    if let Some(name) = extract_tool_name_from_json(&parsed) {
                        let arguments = extract_tool_arguments_from_json(&parsed);
                        let (server, tool) = parse_combined_tool_name(&name);

                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap
                                .get(0)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                            id: None,
                        });
                    }
                } else {
                    // Try XML-style parsing: <name>...</name><arguments>...</arguments>
                    let name_re = Regex::new(r"<name>(.*?)</name>").ok();
                    let args_re = Regex::new(r"(?s)<arguments>(.*?)</arguments>").ok();

                    if let (Some(name_re), Some(args_re)) = (name_re, args_re) {
                        if let Some(name_cap) = name_re.captures(call_content) {
                            let name = name_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                            let arguments = args_re
                                .captures(call_content)
                                .and_then(|c| c.get(1))
                                .and_then(|m| serde_json::from_str::<Value>(m.as_str()).ok())
                                .unwrap_or(Value::Object(serde_json::Map::new()));

                            let (server, tool) = parse_combined_tool_name(name);
                            calls.push(ParsedToolCall {
                                server,
                                tool,
                                arguments,
                                raw: cap
                                    .get(0)
                                    .map(|m| m.as_str().to_string())
                                    .unwrap_or_default(),
                                id: None,
                            });
                        }
                    }
                }
            }
        }
    }

    // Fallback to Hermes parser if no Granite-style calls found
    if calls.is_empty() {
        return parse_hermes_tool_calls(content);
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_granite_tool_call() {
        let content = r#"Let me call the function.
<function_call>{"name": "mcp___read_file", "arguments": {"path": "/tmp/test.txt"}}</function_call>"#;

        let calls = parse_granite_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "mcp");
        assert_eq!(calls[0].tool, "read_file");
    }

    #[test]
    fn test_parse_gemma_function_call_format() {
        let content = r#"<function_call>{"name": "get_product_details", "arguments": {"product_id": "1234"}}</function_call>"#;

        let calls = parse_granite_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_product_details");
        assert_eq!(
            calls[0]
                .arguments
                .get("product_id")
                .and_then(|v| v.as_str()),
            Some("1234")
        );
    }
}
