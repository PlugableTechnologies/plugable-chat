//! Message construction utilities for the agentic loop.
//!
//! This module provides functions for building chat messages with tool calls
//! and tool results in the format expected by different model families.

use crate::protocol::{ChatMessage, OpenAIToolCall, OpenAIToolCallFunction, ParsedToolCall};

/// Create an assistant message, optionally with native tool calls.
///
/// If `use_native_format` is true and all tool calls have IDs, includes a `tool_calls`
/// array in the message. Otherwise, returns a text-only message.
pub fn create_assistant_message_with_tool_calls(
    content: &str,
    calls: &[ParsedToolCall],
    use_native_format: bool,
    system_prompt: Option<String>,
) -> ChatMessage {
    if use_native_format && calls.iter().all(|c| c.id.is_some()) {
        // Native format: include tool_calls array in assistant message
        let tool_calls: Vec<OpenAIToolCall> = calls
            .iter()
            .filter_map(|c| {
                c.id.as_ref().map(|id| OpenAIToolCall {
                    id: id.clone(),
                    call_type: "function".to_string(),
                    function: OpenAIToolCallFunction {
                        name: if c.server == "builtin" || c.server == "unknown" {
                            c.tool.clone()
                        } else {
                            format!("{}___{}", c.server, c.tool)
                        },
                        arguments: serde_json::to_string(&c.arguments).unwrap_or_default(),
                    },
                })
            })
            .collect();

        ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            system_prompt,
            tool_calls: Some(tool_calls),
            tool_call_id: None,
        }
    } else {
        // Text-based format: content only
        ChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            system_prompt,
            tool_calls: None,
            tool_call_id: None,
        }
    }
}

/// Create a tool result message for native OpenAI format.
///
/// The message has role "tool" and includes the tool_call_id to associate
/// this result with the corresponding tool call.
pub fn create_native_tool_result_message(tool_call_id: &str, content: &str) -> ChatMessage {
    ChatMessage {
        role: "tool".to_string(),
        content: content.to_string(),
        system_prompt: None,
        tool_calls: None,
        tool_call_id: Some(tool_call_id.to_string()),
    }
}

/// Check if we should use native tool result format.
///
/// Returns true when native tool calling is enabled AND all tool calls have IDs.
pub fn should_use_native_tool_results(
    native_tool_calling_enabled: bool,
    calls: &[ParsedToolCall],
) -> bool {
    native_tool_calling_enabled && calls.iter().all(|c| c.id.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_create_assistant_message_with_native_tool_calls() {
        let calls = vec![ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT 1"}),
            raw: "".to_string(),
            id: Some("call_123".to_string()),
        }];

        let msg = create_assistant_message_with_tool_calls(
            "Let me query that",
            &calls,
            true,
            None,
        );

        assert_eq!(msg.role, "assistant");
        assert!(msg.tool_calls.is_some());
        let tool_calls = msg.tool_calls.unwrap();
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_123");
        assert_eq!(tool_calls[0].function.name, "sql_select");
    }

    #[test]
    fn test_create_assistant_message_text_based_fallback() {
        let calls = vec![ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT 1"}),
            raw: "".to_string(),
            id: None, // No ID means text-based format
        }];

        let msg = create_assistant_message_with_tool_calls(
            "Let me query that",
            &calls,
            true, // Even with native enabled, no IDs = text format
            None,
        );

        assert_eq!(msg.role, "assistant");
        assert!(msg.tool_calls.is_none());
    }

    #[test]
    fn test_create_native_tool_result_message() {
        let msg = create_native_tool_result_message("call_123", "Query returned 5 rows");

        assert_eq!(msg.role, "tool");
        assert_eq!(msg.content, "Query returned 5 rows");
        assert_eq!(msg.tool_call_id, Some("call_123".to_string()));
    }

    #[test]
    fn test_should_use_native_tool_results() {
        let calls_with_ids = vec![ParsedToolCall {
            server: "builtin".to_string(),
            tool: "test".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
            id: Some("call_1".to_string()),
        }];

        let calls_without_ids = vec![ParsedToolCall {
            server: "builtin".to_string(),
            tool: "test".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
            id: None,
        }];

        assert!(should_use_native_tool_results(true, &calls_with_ids));
        assert!(!should_use_native_tool_results(true, &calls_without_ids));
        assert!(!should_use_native_tool_results(false, &calls_with_ids));
    }
}
