//! Tagged tool call parser.
//!
//! Parses tag-based tool calls such as `[TOOL_CALLS] [{...}]` (Mistral-style).

use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::{
    extract_tool_name_from_json, extract_tool_arguments_from_json, parse_combined_tool_name,
};
use super::json_fixer::parse_json_lenient;

/// Parse tag-based tool calls such as `[TOOL_CALLS] [{...}]` (Mistral-style).
pub fn parse_tagged_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let marker = "[TOOL_CALLS]";

    if let Some(idx) = content.find(marker) {
        let mut payload = content[idx + marker.len()..].trim_start();

        // Trim at closing markers if present
        for end_marker in ["[/TOOL_CALLS]", "[TOOL_RESULTS]"] {
            if let Some(pos) = payload.find(end_marker) {
                payload = &payload[..pos];
                break;
            }
        }

        let trimmed = payload.trim();

        // Attempt parsing as-is, then try without surrounding [] if present
        let parsed = parse_json_lenient(trimmed).or_else(|| {
            let without_brackets = trimmed.trim_matches(|c| c == '[' || c == ']');
            parse_json_lenient(without_brackets)
        });

        if let Some(value) = parsed {
            let entries = match value {
                Value::Array(arr) => arr,
                other => vec![other],
            };

            for entry in entries {
                if let Some(name) = extract_tool_name_from_json(&entry) {
                    let arguments = extract_tool_arguments_from_json(&entry);
                    let (server, tool) = parse_combined_tool_name(&name);

                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw: trimmed.to_string(),
                        id: None,
                    });
                }
            }
        }
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tagged_tool_call_with_single_quotes() {
        let content = "[TOOL_CALLS] [{'name': 'search', 'arguments': {'query': 'AI'}}]";

        let calls = parse_tagged_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "search");
        assert_eq!(
            calls[0].arguments.get("query").and_then(|v| v.as_str()),
            Some("AI")
        );
    }

    #[test]
    fn test_parse_tagged_tool_call_ignores_following_results() {
        let content = "[TOOL_CALLS] [{\"name\": \"calc\", \"arguments\": {\"a\": 1}}][TOOL_RESULTS] placeholder";

        let calls = parse_tagged_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "calc");
        assert_eq!(
            calls[0].arguments.get("a").and_then(|v| v.as_i64()),
            Some(1)
        );
    }
}
