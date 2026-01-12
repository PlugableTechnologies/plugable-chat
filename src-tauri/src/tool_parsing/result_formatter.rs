//! Tool result formatting for different model formats.
//!
//! Formats tool results for injection into the chat history based on the model's
//! expected format.

use crate::protocol::{ParsedToolCall, ToolFormat};
use crate::system_prompt;

/// Success guidance for sql_select - tells model that results have been shown to user
/// Format a tool result for injection into the chat history based on model format
/// 
/// When `is_error` is true and `original_user_prompt` is provided, the error guidance
/// will include a reminder of what the user originally asked, helping the model
/// understand the context for its retry.
///
/// For SQL errors, if `schema_context` is provided, uses the enhanced
/// `build_sql_error_recovery_prompt()` which injects the schema directly into
/// the error response. This is the "Cursor for SQL" approach: small models
/// don't look back in context, so we re-inject what they need.
pub fn format_tool_result(
    call: &ParsedToolCall,
    result: &str,
    is_error: bool,
    tool_format: ToolFormat,
    original_user_prompt: Option<&str>,
    schema_context: Option<&str>,
) -> String {
    let guidance = if is_error {
        // For SQL errors with schema context, use enhanced recovery prompt
        if call.tool == "sql_select" && schema_context.is_some() {
            build_sql_error_recovery_guidance(result, original_user_prompt, schema_context)
        } else {
            system_prompt::build_error_guidance(&call.tool, original_user_prompt)
        }
    } else if call.tool == "sql_select" {
        system_prompt::SQL_SUCCESS_GUIDANCE.to_string()
    } else {
        String::new()
    };

    match tool_format {
        ToolFormat::OpenAI => {
            // OpenAI format - this would typically be a separate message with role "tool"
            // For text-based injection, we use a simple format
            if is_error {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\" error=\"true\">\n{}\n</tool_result>{}",
                    call.server, call.tool, result, guidance
                )
            } else {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\">\n{}\n</tool_result>{}",
                    call.server, call.tool, result, guidance
                )
            }
        }
        ToolFormat::Hermes => {
            // Hermes models expect results in a similar XML format
            if is_error {
                format!(
                    "<tool_response error=\"true\">\n{}\n</tool_response>{}",
                    result, guidance
                )
            } else {
                format!("<tool_response>\n{}\n</tool_response>{}", result, guidance)
            }
        }
        ToolFormat::Gemini => {
            // Gemini uses function_response format
            if is_error {
                format!(
                    "{{\"function_response\": {{\"name\": \"{}___{}\", \"response\": {{\"error\": \"{}\"}}}}}}{}",
                    call.server,
                    call.tool,
                    result.replace('"', "\\\""),
                    guidance
                )
            } else {
                format!(
                    "{{\"function_response\": {{\"name\": \"{}___{}\", \"response\": {{\"result\": \"{}\"}}}}}}{}",
                    call.server,
                    call.tool,
                    result.replace('"', "\\\""),
                    guidance
                )
            }
        }
        ToolFormat::Granite => {
            // Granite uses <function_response> tags
            if is_error {
                format!(
                    "<function_response error=\"true\">\n{}\n</function_response>{}",
                    result, guidance
                )
            } else {
                format!("<function_response>\n{}\n</function_response>{}", result, guidance)
            }
        }
        ToolFormat::Harmony => {
            // Harmony format uses <|start|>tool to={tool_name}<|message|>{result}<|end|>
            if is_error {
                format!(
                    "<|start|>tool to={}<|message|>{{\"error\": \"{}\"}}<|end|>{}",
                    call.tool,
                    result.replace('"', "\\\""),
                    guidance
                )
            } else {
                format!(
                    "<|start|>tool to={}<|message|>{}<|end|>{}",
                    call.tool,
                    result,
                    guidance
                )
            }
        }
        ToolFormat::TextBased => {
            // Generic text-based format
            if is_error {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\" error=\"true\">\n{}\n</tool_result>{}",
                    call.server, call.tool, result, guidance
                )
            } else {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\">\n{}\n</tool_result>{}",
                    call.server, call.tool, result, guidance
                )
            }
        }
    }
}

/// Build SQL error recovery guidance by extracting SQL and error from the result JSON.
/// 
/// This parses the sql_select output format and uses the enhanced recovery prompt
/// that injects schema context directly.
fn build_sql_error_recovery_guidance(
    result: &str,
    original_user_prompt: Option<&str>,
    schema_context: Option<&str>,
) -> String {
    // Try to parse the result as JSON to extract sql_executed and error
    let (sql_executed, error_message) = if let Ok(json) = serde_json::from_str::<serde_json::Value>(result) {
        let sql = json.get("sql_executed")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let error = json.get("error")
            .and_then(|v| v.as_str())
            .unwrap_or(result);
        (sql.to_string(), error.to_string())
    } else {
        // If not JSON, use the raw result as the error message
        (String::new(), result.to_string())
    };
    
    let user_prompt = original_user_prompt.unwrap_or("");
    
    // Use the enhanced recovery prompt with schema injection
    format!(
        "\n\n{}",
        system_prompt::build_sql_error_recovery_prompt(
            &sql_executed,
            &error_message,
            schema_context,
            user_prompt,
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_format_tool_result_hermes() {
        let call = ParsedToolCall {
            server: "test".to_string(),
            tool: "echo".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
            id: None,
        };

        let result = format_tool_result(&call, "Hello, World!", false, ToolFormat::Hermes, None, None);
        assert!(result.contains("<tool_response>"));
        assert!(result.contains("Hello, World!"));
        // Success case should NOT include error guidance
        assert!(!result.contains("TOOL ERROR"));
    }

    #[test]
    fn test_format_tool_result_sql_success_includes_guidance() {
        let call = ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT * FROM users"}),
            raw: "".to_string(),
            id: None,
        };

        let sql_result = r#"{"success": true, "columns": ["id", "name"], "rows": [[1, "Alice"]], "row_count": 1}"#;

        let result = format_tool_result(&call, sql_result, false, ToolFormat::Hermes, None, None);
        assert!(
            result.contains("already been displayed to the user"),
            "Should tell model results were shown to user, got: {}",
            result
        );
    }

    #[test]
    fn test_format_harmony_tool_result_success() {
        let call = ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT 1"}),
            raw: "".to_string(),
            id: None,
        };
        let result = format_tool_result(
            &call,
            r#"{"rows": [[1]], "columns": ["?column?"]}"#,
            false,
            ToolFormat::Harmony,
            None,
            None,
        );
        assert!(result.contains("<|start|>tool to=sql_select"), "Should use harmony format");
        assert!(result.contains("<|message|>"), "Should contain message token");
        assert!(result.contains("<|end|>"), "Should contain end token");
        assert!(result.contains("rows"), "Should contain result data");
    }

    #[test]
    fn test_format_harmony_tool_result_error() {
        let call = ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT * FROM nonexistent"}),
            raw: "".to_string(),
            id: None,
        };
        let result = format_tool_result(
            &call,
            "Table not found: nonexistent",
            true,
            ToolFormat::Harmony,
            None,
            None,
        );
        assert!(result.contains("<|start|>tool to=sql_select"), "Should use harmony format");
        assert!(result.contains("error"), "Should contain error field");
    }
}
