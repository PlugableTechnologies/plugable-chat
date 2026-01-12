//! Request body construction for Foundry API calls.
//!
//! This module handles:
//! - Building model-family-specific chat request bodies
//! - Converting chat messages to Responses API format

use serde_json::{json, Value};
use crate::protocol::{ChatMessage, ModelFamily, OpenAITool};

/// Build a chat request body with model-family-specific parameters
pub fn build_foundry_chat_request_body(
    model: &str,
    family: ModelFamily,
    messages: &[ChatMessage],
    tools: &Option<Vec<OpenAITool>>,
    use_native_tools: bool,
    supports_reasoning: bool,
    supports_reasoning_effort: bool,
    reasoning_effort: &str,
    use_responses_api: bool,
) -> Value {
    let mut body = if use_responses_api {
        json!({
            "model": model,
            "input": convert_chat_messages_to_foundry_format(messages),
            "stream": true,
        })
    } else {
        json!({
            "model": model,
            "messages": messages,
            "stream": true,
        })
    };

    // Note: EP (execution provider) parameter is not passed to completions
    // as it didn't work reliably. Foundry will auto-select the best EP.

    // Add model-family-specific parameters
    match family {
        ModelFamily::GptOss => {
            // GPT-OSS models: standard OpenAI-compatible parameters
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(16384);
            body["temperature"] = json!(0.7);

            if use_native_tools {
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Phi => {
            // Phi models: may support reasoning_effort
            if supports_reasoning && supports_reasoning_effort {
                println!(
                    "[FoundryActor] Phi model with reasoning, using effort: {}",
                    reasoning_effort
                );
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
                // Note: Reasoning models typically don't use tools in the same request
            } else {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(16384);
                if use_native_tools {
                    if let Some(tool_list) = tools {
                        body["tools"] = json!(tool_list);
                    }
                }
            }
        }
        ModelFamily::Gemma => {
            // Gemma models: support temperature and top_k
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(8192);
            body["temperature"] = json!(0.7);
            // Gemma supports top_k which is useful for controlling randomness
            body["top_k"] = json!(40);

            if use_native_tools {
                // Gemma may use a different tool format, but Foundry handles this
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Granite => {
            // IBM Granite models: support repetition_penalty
            body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                json!(8192);
            body["temperature"] = json!(0.7);
            // Granite models benefit from repetition penalty
            body["repetition_penalty"] = json!(1.05);

            if supports_reasoning {
                // Granite reasoning models use <|thinking|> tags internally
                println!("[FoundryActor] Granite model with reasoning support");
            }

            if use_native_tools {
                if let Some(tool_list) = tools {
                    body["tools"] = json!(tool_list);
                }
            }
        }
        ModelFamily::Generic => {
            // Generic/unknown models: use safe defaults
            if supports_reasoning && supports_reasoning_effort {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(8192);
                body["reasoning_effort"] = json!(reasoning_effort);
            } else {
                body[if use_responses_api { "max_output_tokens" } else { "max_tokens" }] =
                    json!(16384);
                if use_native_tools {
                    if let Some(tool_list) = tools {
                        body["tools"] = json!(tool_list);
                    }
                }
            }
        }
    }

    body
}

/// Convert OpenAI chat messages into Responses API input blocks (text-only)
pub fn convert_chat_messages_to_foundry_format(messages: &[ChatMessage]) -> Vec<Value> {
    messages
        .iter()
        .map(|msg| {
            json!({
                "role": msg.role,
                "content": [
                    {
                        "type": "text",
                        "text": msg.content
                    }
                ]
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convert_chat_messages_to_foundry_format_wraps_text() {
        let messages = vec![ChatMessage {
            role: "user".to_string(),
            content: "hi there".to_string(),
            system_prompt: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let input = convert_chat_messages_to_foundry_format(&messages);
        assert_eq!(input.len(), 1);
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[0]["content"][0]["text"], "hi there");
    }
}
