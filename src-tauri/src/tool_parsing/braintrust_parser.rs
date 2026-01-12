//! Braintrust function call parser.
//!
//! Parses Braintrust-style function calls: `<function=get_weather>{"location": "Tokyo"}</function>`
//! This format is used by some Llama 3.x recipes.

use regex::Regex;
use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::parse_combined_tool_name;
use super::json_fixer::repair_malformed_json;

/// Parse Braintrust-style function calls: <function=get_weather>{"location": "Tokyo"}</function>
/// This format is used by some Llama 3.x recipes
pub fn parse_braintrust_function_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match <function=name>{...}</function>
    let re = Regex::new(r"(?s)<function=([^>]+)>\s*(\{.*?\})\s*</function>").ok();

    if let Some(re) = re {
        for cap in re.captures_iter(content) {
            let function_name = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
            let json_str = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("{}");

            if function_name.is_empty() {
                continue;
            }

            let fixed_json = repair_malformed_json(json_str);
            let arguments = serde_json::from_str::<Value>(&fixed_json)
                .unwrap_or(Value::Object(serde_json::Map::new()));

            let raw = cap
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
            let (server, tool) = parse_combined_tool_name(function_name);

            println!(
                "[parse_braintrust_function_calls] Found tool call: {} (server: {})",
                tool, server
            );

            calls.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw,
                id: None,
            });
        }
    }

    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_braintrust_function_format() {
        let content = r#"Let me check the weather.
<function=get_weather>{"location": "Tokyo, JP"}</function>
The weather is..."#;

        let calls = parse_braintrust_function_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_weather");
        assert_eq!(
            calls[0].arguments.get("location").and_then(|v| v.as_str()),
            Some("Tokyo, JP")
        );
    }
}
