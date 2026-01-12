//! Gemini format tool call parser.
//!
//! Parses Gemini's function_call format in model responses.

use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::parse_combined_tool_name;
use super::hermes_parser::parse_hermes_tool_calls;
use super::json_fixer::parse_json_lenient;

/// Parse Gemini function_call format
pub fn parse_gemini_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Try to parse the content as JSON and look for function_call
    // Note: parse_json_lenient may unwrap {"function_call": {...}} to just {...}
    // so we need to handle both cases
    if let Some(parsed) = parse_json_lenient(content.trim()) {
        // Case 1: The function_call wrapper was preserved
        if let Some(function_call) = parsed.get("function_call") {
            if let Some(name) = function_call.get("name").and_then(|v| v.as_str()) {
                let arguments = function_call
                    .get("args")
                    .or_else(|| function_call.get("arguments"))
                    .cloned()
                    .unwrap_or(Value::Object(serde_json::Map::new()));

                let (server, tool) = parse_combined_tool_name(name);
                calls.push(ParsedToolCall {
                    server,
                    tool,
                    arguments,
                    raw: content.to_string(),
                    id: None,
                });
            }
        }
        // Case 2: The function_call wrapper was unwrapped by parse_json_lenient
        // Check if the parsed object directly has a "name" field (from unwrapping)
        else if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
            let arguments = parsed
                .get("args")
                .or_else(|| parsed.get("arguments"))
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let (server, tool) = parse_combined_tool_name(name);
            calls.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw: content.to_string(),
                id: None,
            });
        }
    }

    // Fallback to Hermes parser if no Gemini-style calls found
    if calls.is_empty() {
        return parse_hermes_tool_calls(content);
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_gemini_function_call() {
        let content = r#"{"function_call": {"name": "get_weather", "args": {"location": "Tokyo"}}}"#;
        let calls = parse_gemini_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_weather");
        assert_eq!(
            calls[0].arguments.get("location").and_then(|v| v.as_str()),
            Some("Tokyo")
        );
    }
}
