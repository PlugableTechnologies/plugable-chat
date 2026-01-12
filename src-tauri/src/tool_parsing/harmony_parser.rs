//! Harmony format tool call parser.
//!
//! Harmony format uses special tokens for structured output:
//! - `<|channel|>commentary to={tool_name} <|constrain|>json<|message|>{args}<|call|>`
//!
//! The tool name is in the channel header (after `to=`), not inside the JSON.
//! The JSON content after `<|message|>` is the arguments directly (not wrapped).

use regex::Regex;
use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::parse_combined_tool_name;
use super::json_fixer::{repair_malformed_json, parse_json_lenient};
use super::hermes_parser::parse_hermes_tool_calls;

/// Parse harmony-format tool calls from gpt-oss models.
pub fn parse_harmony_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match: <|channel|>commentary to=TOOL_NAME ... <|message|>ARGS<|call|> or <|end|>
    // The tool name can include server___tool format
    // Note: The pipe character | in tokens like <|channel|> must be escaped as \|
    // Note: Rust regex doesn't support look-ahead, so we match up to terminators explicitly
    let pattern = r"(?s)<\|channel\|>commentary\s+to=(\S+)(?:\s+<\|constrain\|>\w+)?<\|message\|>(.*?)(?:<\|call\|>|<\|end\|>|<\|channel\|>|$)";
    let re = Regex::new(pattern);

    if let Ok(re) = re {
        for cap in re.captures_iter(content) {
            let tool_name = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let args_str = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("{}");

            if tool_name.is_empty() {
                continue;
            }

            // Parse arguments JSON directly (not wrapped in {"name": ..., "arguments": ...})
            let fixed_json = repair_malformed_json(args_str);
            let arguments = parse_json_lenient(&fixed_json)
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let (server, tool) = parse_combined_tool_name(tool_name);

            let raw = cap
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            calls.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw,
                id: None,
            });
        }
    }

    // Fallback: if no harmony tool calls found, try Hermes parsing
    // (in case model outputs mixed format)
    if calls.is_empty() {
        return parse_hermes_tool_calls(content);
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_harmony_tool_call_basic() {
        let content = r#"<|channel|>commentary to=sql_select <|constrain|>json<|message|>{"sql":"SELECT * FROM users"}<|call|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse basic harmony tool call");
        assert_eq!(calls[0].server, "unknown");
        assert_eq!(calls[0].tool, "sql_select");
        assert_eq!(calls[0].arguments["sql"], "SELECT * FROM users");
    }

    #[test]
    fn test_parse_harmony_tool_call_with_server_prefix() {
        let content = r#"<|channel|>commentary to=builtin___python_execution <|constrain|>json<|message|>{"code":"print('hello')"}<|call|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse harmony tool call with server prefix");
        assert_eq!(calls[0].server, "builtin");
        assert_eq!(calls[0].tool, "python_execution");
        assert_eq!(calls[0].arguments["code"], "print('hello')");
    }

    #[test]
    fn test_parse_harmony_with_end_instead_of_call() {
        let content = r#"<|channel|>commentary to=sql_select <|constrain|>json<|message|>{"sql":"SELECT 1"}<|end|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse harmony with <|end|> terminator");
        assert_eq!(calls[0].tool, "sql_select");
    }

    #[test]
    fn test_parse_harmony_multiple_channels() {
        let content = r#"<|channel|>analysis<|message|>Let me search for pickpocket crimes...<|end|><|channel|>commentary to=sql_select <|constrain|>json<|message|>{"sql":"SELECT COUNT(*)"}<|call|><|channel|>final<|message|>Here are the results...<|end|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should only parse commentary with to= as tool call");
        assert_eq!(calls[0].tool, "sql_select");
    }

    #[test]
    fn test_parse_harmony_complex_sql() {
        let content = r#"<|channel|>commentary to=sql_select <|constrain|>json<|message|>{"sql":"SELECT COUNT(*) AS total_pickpockets FROM main.chicago_crimes WHERE description LIKE '%PICKPOCKET%'"}<|call|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "sql_select");
        assert!(calls[0].arguments["sql"].as_str().unwrap().contains("PICKPOCKET"));
    }

    #[test]
    fn test_parse_harmony_no_constrain() {
        let content = r#"<|channel|>commentary to=tool_search<|message|>{"query":"weather tools"}<|call|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse without <|constrain|> token");
        assert_eq!(calls[0].tool, "tool_search");
    }

    #[test]
    fn test_parse_harmony_multiple_tool_calls() {
        let content = r#"<|channel|>commentary to=schema_search <|constrain|>json<|message|>{"query":"users"}<|call|><|channel|>commentary to=sql_select <|constrain|>json<|message|>{"sql":"SELECT * FROM users"}<|call|>"#;
        let calls = parse_harmony_tool_calls(content);
        assert_eq!(calls.len(), 2, "Should parse multiple harmony tool calls");
        assert_eq!(calls[0].tool, "schema_search");
        assert_eq!(calls[1].tool, "sql_select");
    }
}
