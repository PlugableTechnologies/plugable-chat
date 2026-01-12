//! Hermes-style tool call parser.
//!
//! Parses tool calls in the format: `<tool_call>{"name": "...", "arguments": {...}}</tool_call>`
//! Used by Phi, Qwen, and as a fallback for other formats.
//!
//! Also handles:
//! - `tool_name`/`tool_args` format (GPT-OSS legacy)
//! - `parameters` as alias for `arguments` (Llama)
//! - Case-insensitive tags (<Tool_Call>, <TOOL_CALL>)
//! - Tags with attributes (<tool_call id="1">)
//! - Common typos (<toolcall>, <tool-call>, <tool_calls>)

use regex::Regex;
use crate::protocol::ParsedToolCall;
use super::common::{
    extract_tool_name_from_json, extract_tool_arguments_from_json, 
    parse_combined_tool_name, extract_tool_call_by_regex,
};
use super::json_fixer::{
    repair_malformed_json, parse_json_lenient, extract_balanced_json_braces,
    find_json_objects_in_content,
};
use super::tagged_parser::parse_tagged_tool_calls;
use super::braintrust_parser::parse_braintrust_function_calls;
use super::markdown_json_parser::parse_markdown_json_tool_calls;
use super::pythonic_parser::{parse_pythonic_code_block_tool_calls, parse_pythonic_tool_calls};

/// Parse Hermes-style tool calls: <tool_call>{"name": "...", "arguments": {...}}</tool_call>
/// This is used by Phi, Qwen, and as a fallback for other formats.
pub fn parse_hermes_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match <tool_call> variants with:
    // - Case insensitivity ((?i))
    // - Optional whitespace around tag name
    // - Optional attributes in opening tag ([^>]*)
    // - Common typos/variants (tool_call|toolcall|tool-call|tool_calls)
    // - Flexible closing tag
    let re = Regex::new(r"(?si)<\s*(tool_call|toolcall|tool-call|tool_calls)\s*[^>]*>\s*(.*?)\s*</\s*(tool_call|toolcall|tool-call|tool_calls)\s*>").unwrap();

    // Also check for unclosed tool calls (streaming) - case insensitive
    let unclosed_re = Regex::new(r"(?si)<\s*(tool_call|toolcall|tool-call|tool_calls)\s*[^>]*>\s*(\{.*)").ok();

    for cap in re.captures_iter(content) {
        // Group 1: tag name variant, Group 2: JSON content, Group 3: closing tag name
        if let Some(json_match) = cap.get(2) {
            let json_str = json_match.as_str().trim();
            // Strip trailing non-JSON characters (e.g., stray `>` from malformed tags)
            let json_str = json_str.trim_end_matches(|c: char| c == '>' || c == '/');
            let fixed_json = repair_malformed_json(json_str);

            if let Some(parsed) = parse_json_lenient(&fixed_json) {
                let raw = cap
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                // Try Format 1: {"server": "...", "tool": "...", "arguments": {...}}
                if let (Some(server), Some(tool)) = (
                    parsed.get("server").and_then(|v| v.as_str()),
                    parsed.get("tool").and_then(|v| v.as_str()),
                ) {
                    let arguments = extract_tool_arguments_from_json(&parsed);

                    calls.push(ParsedToolCall {
                        server: server.to_string(),
                        tool: tool.to_string(),
                        arguments,
                        raw,
                        id: None,
                    });
                    continue;
                }

                // Try Format 2: {"name": "...", "arguments": {...}} or {"tool_name": "...", "tool_args": {...}}
                if let Some(name) = extract_tool_name_from_json(&parsed) {
                    let arguments = extract_tool_arguments_from_json(&parsed);

                    let (server, tool) = parse_combined_tool_name(&name);
                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw,
                        id: None,
                    });
                }
            }
        }
    }

    // If no tool calls found, check for unclosed tool calls (streaming)
    if calls.is_empty() {
        if let Some(unclosed_re) = unclosed_re {
            if let Some(cap) = unclosed_re.captures(content) {
                // Group 1: tag name variant, Group 2: JSON content (starting with {)
                if let Some(json_match) = cap.get(2) {
                    let json_str = json_match.as_str().trim();

                    if let Some(balanced_json) = extract_balanced_json_braces(json_str) {
                        let fixed_json = repair_malformed_json(&balanced_json);

                        if let Some(parsed) = parse_json_lenient(&fixed_json) {
                            if let Some(name) = extract_tool_name_from_json(&parsed) {
                                let arguments = extract_tool_arguments_from_json(&parsed);

                                let (server, tool) = parse_combined_tool_name(&name);
                                calls.push(ParsedToolCall {
                                    server,
                                    tool,
                                    arguments,
                                    raw: format!("<tool_call>{}</tool_call>", balanced_json),
                                    id: None,
                                });
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: tag-based formats like [TOOL_CALLS] ... (Mistral-style)
    if calls.is_empty() {
        calls = parse_tagged_tool_calls(content);
    }

    // Fallback: check for Braintrust-style <function=name>{...}</function> format (Llama)
    if calls.is_empty() {
        calls = parse_braintrust_function_calls(content);
    }

    // Fallback: check for markdown code blocks containing JSON tool calls
    // This handles smaller models that output ```json {...} ``` instead of <tool_call>
    if calls.is_empty() {
        calls = parse_markdown_json_tool_calls(content);
    }

    // Fallback: check for Pythonic function calls in code blocks
    // This handles models that output ```plaintext tool_name(...) ``` or similar
    if calls.is_empty() {
        calls = parse_pythonic_code_block_tool_calls(content);
    }

    // Fallback: try bare Pythonic function calls (not in code blocks)
    if calls.is_empty() {
        calls = parse_pythonic_tool_calls(content);
    }

    // Fallback: find JSON objects anywhere in content and try to parse them
    if calls.is_empty() {
        for json_str in find_json_objects_in_content(content) {
            if let Some(parsed) = parse_json_lenient(&json_str) {
                if let Some(name) = extract_tool_name_from_json(&parsed) {
                    let arguments = extract_tool_arguments_from_json(&parsed);
                    let (server, tool) = parse_combined_tool_name(&name);
                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw: json_str,
                        id: None,
                    });
                }
            }
        }
    }

    // Last resort: regex-based field extraction
    if calls.is_empty() {
        if let Some(call) = extract_tool_call_by_regex(content) {
            calls.push(call);
        }
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_parse_hermes_tool_call() {
        let content = r#"I'll use the tool.
<tool_call>{"name": "server1___get_data", "arguments": {"id": 123}}</tool_call>
Done."#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "server1");
        assert_eq!(calls[0].tool, "get_data");
    }

    #[test]
    fn test_parse_hermes_allows_single_quoted_json() {
        let content = "<tool_call>{'name': 'echo', 'arguments': {'text': 'hi'}}</tool_call>";

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "echo");
        assert_eq!(
            calls[0].arguments.get("text").and_then(|v| v.as_str()),
            Some("hi")
        );
    }

    #[test]
    fn test_parse_hermes_with_stray_closing_bracket() {
        let content = r#"<tool_call>{"name": "sql_select", "arguments": {"sql": "SELECT SUM(total_sale) FROM table WHERE period >= '2025-09-01' AND period < '2025-10-01'"}}></tool_call>"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse tool call with stray > character");
        assert_eq!(calls[0].tool, "sql_select");
        assert!(calls[0].arguments.get("sql").is_some(), "Should extract sql argument");
    }

    #[test]
    fn test_parse_hermes_case_insensitive() {
        let content = r#"<TOOL_CALL>{"name": "test", "arguments": {}}</TOOL_CALL>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse uppercase TOOL_CALL");
        assert_eq!(calls[0].tool, "test");
    }

    #[test]
    fn test_parse_hermes_typo_toolcall() {
        let content = r#"<toolcall>{"name": "test", "arguments": {}}</toolcall>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse typo 'toolcall'");
        assert_eq!(calls[0].tool, "test");
    }

    #[test]
    fn test_parse_hermes_with_attributes() {
        let content = r#"<tool_call id="1" type="function">{"name": "test", "arguments": {}}</tool_call>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse tool_call with attributes");
        assert_eq!(calls[0].tool, "test");
    }

    #[test]
    fn test_parse_gpt_oss_legacy_format() {
        let content = r#"<tool_call>{"tool_name": "get_weather", "tool_args": {"location": "Seattle"}}</tool_call>"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_weather");
        assert_eq!(
            calls[0].arguments.get("location").and_then(|v| v.as_str()),
            Some("Seattle")
        );
    }

    #[test]
    fn test_parse_llama_parameters_format() {
        let content = r#"<tool_call>{"name": "search", "parameters": {"query": "rust programming"}}</tool_call>"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "search");
        assert_eq!(
            calls[0].arguments.get("query").and_then(|v| v.as_str()),
            Some("rust programming")
        );
    }

    #[test]
    fn test_parse_with_python_booleans_in_arguments() {
        let content = r#"<tool_call>{"name": "test", "arguments": {"enabled": True, "disabled": False}}</tool_call>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse with Python booleans");
        assert_eq!(calls[0].arguments.get("enabled").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(calls[0].arguments.get("disabled").and_then(|v| v.as_bool()), Some(false));
    }

    #[test]
    fn test_json_with_leading_garbage() {
        let content = r#"Here is some explanation text and then {"name": "test", "arguments": {}} more text"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should find JSON in text via fallback");
        assert_eq!(calls[0].tool, "test");
    }
}
