//! Tool Parsing Module
//!
//! Handles model-specific tool calling formats for different model families.
//! Each model family may have different ways of:
//! - Receiving tool definitions in the request
//! - Outputting tool calls in the response
//! - Receiving tool results
//!
//! Supported formats:
//! - OpenAI: Standard tool_calls array in response, "tool" role for results
//! - Hermes: <tool_call>JSON</tool_call> XML format (Phi, Qwen)
//! - Harmony: <|channel|>commentary to={tool}... format (GPT-OSS)
//! - Gemini: function_call in response, "function" role for results  
//! - Granite: <function_call>XML</function_call> format
//!
//! ## Module Structure
//! - `json_fixer`: JSON repair utilities for malformed LLM output
//! - `common`: Shared utilities (tool name extraction, argument parsing)
//! - `hermes_parser`: Hermes-style <tool_call> parsing
//! - `harmony_parser`: Harmony format parsing
//! - `gemini_parser`: Gemini function_call parsing
//! - `granite_parser`: Granite <function_call> parsing
//! - `tagged_parser`: [TOOL_CALLS] style parsing
//! - `braintrust_parser`: <function=name> style parsing
//! - `markdown_json_parser`: ```json code block parsing
//! - `json_parser`: Pure JSON parsing
//! - `pythonic_parser`: Pythonic function call parsing
//! - `python_detector`: Python code detection for Code Mode
//! - `result_formatter`: Tool result formatting for different models

// Core utilities
pub mod json_fixer;
pub mod common;

// Format-specific parsers
pub mod hermes_parser;
pub mod harmony_parser;
pub mod gemini_parser;
pub mod granite_parser;
pub mod tagged_parser;
pub mod braintrust_parser;
pub mod markdown_json_parser;
pub mod json_parser;
pub mod pythonic_parser;

// Python detection
pub mod python_detector;

// Result formatting
pub mod result_formatter;

use serde_json::{json, Value};

use crate::protocol::{ModelFamily, OpenAITool, ParsedToolCall, ToolFormat};
use crate::settings::{ToolCallFormatConfig, ToolCallFormatName};

// Re-export primary functions that were in tool_adapters.rs
pub use common::parse_combined_tool_name;
pub use python_detector::{detect_python_code, DetectedPythonCode};
pub use result_formatter::format_tool_result;

// Re-export parsers for direct use
pub use hermes_parser::parse_hermes_tool_calls;
pub use harmony_parser::parse_harmony_tool_calls;
pub use gemini_parser::parse_gemini_tool_calls;
pub use granite_parser::parse_granite_tool_calls;

/// Format tools for a specific model family's expected input format.
/// Most models accept OpenAI-compatible tool definitions, but some need adjustments.
pub fn format_tools_for_model(
    tools: &[OpenAITool],
    _family: ModelFamily,
    tool_format: ToolFormat,
) -> Value {
    match tool_format {
        ToolFormat::OpenAI | ToolFormat::Hermes => {
            // OpenAI and Hermes use the same tool definition format
            json!(tools)
        }
        ToolFormat::Gemini => {
            // Gemini uses a slightly different format with function_declarations
            let function_declarations: Vec<Value> = tools
                .iter()
                .map(|t| {
                    let mut decl = json!({
                        "name": t.function.name,
                    });
                    if let Some(desc) = &t.function.description {
                        decl["description"] = json!(desc);
                    }
                    if let Some(params) = &t.function.parameters {
                        decl["parameters"] = params.clone();
                    }
                    decl
                })
                .collect();

            json!([{
                "function_declarations": function_declarations
            }])
        }
        ToolFormat::Granite => {
            // Granite uses a similar format to OpenAI but may need schema adjustments
            json!(tools)
        }
        ToolFormat::Harmony => {
            // Harmony (gpt-oss) uses OpenAI-compatible tool definitions
            json!(tools)
        }
        ToolFormat::TextBased => {
            // No native tool calling - return empty array
            // Text-based tools are handled via system prompt
            json!([])
        }
    }
}

/// Parse tool calls from a model response based on the model's tool format.
/// Returns a vector of ParsedToolCall structs.
pub fn parse_tool_calls_for_model_profile(
    response: &str,
    _family: ModelFamily,
    tool_format: ToolFormat,
    formats: &ToolCallFormatConfig,
    primary: ToolCallFormatName,
) -> Vec<ParsedToolCall> {
    // Build an ordered list starting with the primary, followed by the other enabled formats.
    let mut ordered: Vec<ToolCallFormatName> = vec![primary];
    for fmt in &formats.enabled {
        if *fmt != primary && !ordered.contains(fmt) {
            ordered.push(*fmt);
        }
    }

    for fmt in ordered {
        let calls = match fmt {
            ToolCallFormatName::Hermes => hermes_parser::parse_hermes_tool_calls(response),
            ToolCallFormatName::Mistral => tagged_parser::parse_tagged_tool_calls(response),
            ToolCallFormatName::Pythonic => pythonic_parser::parse_pythonic_tool_calls(response),
            ToolCallFormatName::PureJson => json_parser::parse_pure_json_tool_calls(response),
            // Native and CodeMode are handled via structured response or python_execution
            ToolCallFormatName::Native | ToolCallFormatName::CodeMode => Vec::new(),
        };
        if !calls.is_empty() {
            return calls;
        }
    }

    // Fallback to model-specific parsing only if the format is enabled.
    match tool_format {
        ToolFormat::OpenAI | ToolFormat::Hermes => {
            if formats.is_enabled(ToolCallFormatName::Hermes) {
                hermes_parser::parse_hermes_tool_calls(response)
            } else {
                Vec::new()
            }
        }
        ToolFormat::Gemini => {
            if formats.is_enabled(ToolCallFormatName::Hermes)
                || formats.is_enabled(ToolCallFormatName::PureJson)
            {
                gemini_parser::parse_gemini_tool_calls(response)
            } else {
                Vec::new()
            }
        }
        ToolFormat::Harmony => {
            // gpt-oss harmony format - always try to parse
            // Harmony uses native format so we don't check enabled formats
            harmony_parser::parse_harmony_tool_calls(response)
        }
        ToolFormat::Granite | ToolFormat::TextBased => {
            if formats.is_enabled(ToolCallFormatName::Mistral)
                || formats.is_enabled(ToolCallFormatName::Hermes)
            {
                granite_parser::parse_granite_tool_calls(response)
            } else {
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tool_calls_prefers_primary_enabled_format() {
        let formats = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::Pythonic, ToolCallFormatName::Hermes],
            primary: ToolCallFormatName::Pythonic,
        };

        let calls = parse_tool_calls_for_model_profile(
            "builtin___echo(text=\"hi\")",
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            &formats,
            formats.primary,
        );

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "builtin");
        assert_eq!(calls[0].tool, "echo");
    }

    #[test]
    fn parse_tool_calls_skips_disabled_formats() {
        let formats = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::Pythonic],
            primary: ToolCallFormatName::Pythonic,
        };

        let calls = parse_tool_calls_for_model_profile(
            "<tool_call>{\"name\": \"builtin___echo\", \"arguments\": {\"text\": \"hi\"}}</tool_call>",
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            &formats,
            formats.primary,
        );

        assert!(calls.is_empty());
    }

    #[test]
    fn parse_tool_calls_supports_mistral_when_enabled() {
        let formats = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::Mistral],
            primary: ToolCallFormatName::Mistral,
        };
        let content = r#"[TOOL_CALLS] [{"name": "builtin___echo", "arguments": {"text": "hi"}}]"#;

        let calls = parse_tool_calls_for_model_profile(
            content,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            &formats,
            formats.primary,
        );

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "builtin");
        assert_eq!(calls[0].tool, "echo");
    }

    #[test]
    fn parse_tool_calls_supports_pure_json_when_enabled() {
        let formats = ToolCallFormatConfig {
            enabled: vec![ToolCallFormatName::PureJson],
            primary: ToolCallFormatName::PureJson,
        };
        let content = r#"{"tool": "builtin___echo", "args": {"text": "hi"}}"#;

        let calls = parse_tool_calls_for_model_profile(
            content,
            ModelFamily::GptOss,
            ToolFormat::Hermes,
            &formats,
            formats.primary,
        );

        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "builtin");
        assert_eq!(calls[0].tool, "echo");
    }
}
