//! Markdown JSON code block parser.
//!
//! Parses tool calls from markdown JSON code blocks.
//! Handles formats like:
//! ```json
//! {"name": "tool_name", "arguments": {...}}
//! ```
//! This is a fallback for smaller models that ignore <tool_call> format instructions.

use regex::Regex;

use crate::protocol::ParsedToolCall;
use super::common::{
    extract_tool_name_from_json, extract_tool_arguments_from_json, parse_combined_tool_name,
};
use super::json_fixer::parse_json_lenient;

/// Parse tool calls from markdown JSON code blocks.
/// Handles formats like:
/// ```json
/// {"name": "tool_name", "arguments": {...}}
/// {"tool_name": "...", "tool_args": {...}}  // GPT-OSS
/// {"name": "...", "parameters": {...}}       // Llama
/// ```
/// This is a fallback for smaller models that ignore <tool_call> format instructions.
pub fn parse_markdown_json_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match markdown code blocks: ```json ... ``` or ``` ... ```
    // (?s) for DOTALL mode, optional language specifier (json, etc.)
    // The \s* after the language handles newlines and whitespace before content
    let code_block_re = Regex::new(r"(?s)```(?:json)?\s*(.*?)\s*```").unwrap();

    for cap in code_block_re.captures_iter(content) {
        if let Some(json_match) = cap.get(1) {
            let json_str = json_match.as_str().trim();

            // Skip if it doesn't look like a tool call JSON (must have name-like field)
            if !json_str.contains("\"name\"") && !json_str.contains("\"tool_name\"") {
                continue;
            }

            // Use parse_json_lenient directly - it has internal fallbacks
            // Don't pre-apply repair_malformed_json as it can corrupt valid JSON with newlines
            if let Some(parsed) = parse_json_lenient(json_str) {
                let raw = cap
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                // Check if this looks like a tool call (has name-like field)
                if let Some(name) = extract_tool_name_from_json(&parsed) {
                    // Skip if it doesn't look like a tool name (e.g., just random JSON)
                    // Tool names should be simple identifiers, not long strings
                    if name.len() > 100 || name.contains('\n') {
                        continue;
                    }

                    let arguments = extract_tool_arguments_from_json(&parsed);
                    let (server, tool) = parse_combined_tool_name(&name);

                    println!("[parse_markdown_json_tool_calls] Found tool call in code block: {} (server: {})", tool, server);

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

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_markdown_json_tool_call() {
        let content = r#"To calculate 17 * 23 + 456, I'll use the python_execution tool.

```json
{
  "name": "python_execution",
  "arguments": {
    "code": ["result = 17 * 23 + 456", "print(f'Answer: {result}')"]
  }
}
```
"#;

        let calls = parse_markdown_json_tool_calls(content);
        assert_eq!(
            calls.len(),
            1,
            "Expected 1 tool call, found {}",
            calls.len()
        );
        assert_eq!(calls[0].server, "unknown");
        assert_eq!(calls[0].tool, "python_execution");
        assert!(calls[0].arguments.get("code").is_some());
    }

    #[test]
    fn test_parse_markdown_json_without_language() {
        let content = r#"
```
{"name": "test_tool", "arguments": {"param": "value"}}
```
"#;

        let calls = parse_markdown_json_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "test_tool");
    }

    #[test]
    fn test_parse_markdown_json_ignores_non_tool_json() {
        let content = r#"Here's some config:

```json
{
  "database": "postgres",
  "port": 5432
}
```
"#;

        let calls = parse_markdown_json_tool_calls(content);
        assert_eq!(
            calls.len(),
            0,
            "Should not parse non-tool JSON as tool calls"
        );
    }

    #[test]
    fn test_parse_markdown_gpt_oss_format() {
        let content = r#"
```json
{"tool_name": "calculate", "tool_args": {"expression": "2 + 2"}}
```
"#;

        let calls = parse_markdown_json_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "calculate");
        assert_eq!(
            calls[0]
                .arguments
                .get("expression")
                .and_then(|v| v.as_str()),
            Some("2 + 2")
        );
    }
}
