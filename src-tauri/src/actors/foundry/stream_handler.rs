//! Stream handling for Foundry API responses.
//!
//! This module handles:
//! - Accumulating OpenAI-style streaming tool calls
//! - Extracting text from Chat Completions and Responses API payloads

use std::collections::HashMap;
use serde_json::Value;
use crate::protocol::ParsedToolCall;
use crate::tool_parsing::parse_combined_tool_name;

/// Accumulator for OpenAI-style streaming tool calls.
///
/// In the OpenAI streaming format, tool calls arrive incrementally:
/// - First chunk contains `id`, `type`, and `function.name`
/// - Subsequent chunks contain `function.arguments` fragments
/// - Multiple tool calls are indexed by their `index` field
#[derive(Default)]
pub struct StreamingToolCalls {
    /// Map of index -> (id, name, accumulated_arguments)
    calls: HashMap<usize, (String, String, String)>,
}

impl StreamingToolCalls {
    /// Process a delta.tool_calls array from a streaming chunk
    pub fn process_streaming_tool_call_delta(&mut self, tool_calls: &[Value]) {
        for tc in tool_calls {
            let index = tc["index"].as_u64().unwrap_or(0) as usize;
            let entry = self
                .calls
                .entry(index)
                .or_insert_with(|| (String::new(), String::new(), String::new()));

            // First chunk has id/name
            if let Some(id) = tc["id"].as_str() {
                entry.0 = id.to_string();
            }
            if let Some(name) = tc["function"]["name"].as_str() {
                entry.1 = name.to_string();
            }
            // Accumulate arguments (streamed incrementally)
            if let Some(args) = tc["function"]["arguments"].as_str() {
                entry.2.push_str(args);
            }
        }
    }

    /// Check if any tool calls have been accumulated
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Convert accumulated tool calls to ParsedToolCall format
    pub fn into_parsed_calls(self) -> Vec<ParsedToolCall> {
        let mut result = Vec::new();

        // Sort by index to maintain order
        let mut indexed: Vec<_> = self.calls.into_iter().collect();
        indexed.sort_by_key(|(idx, _)| *idx);

        for (_index, (tool_call_id, name, arguments_str)) in indexed {
            // Skip entries without a name (incomplete)
            if name.is_empty() {
                continue;
            }

            // Parse the accumulated arguments JSON
            let arguments = if arguments_str.is_empty() {
                Value::Object(serde_json::Map::new())
            } else {
                serde_json::from_str(&arguments_str).unwrap_or_else(|e| {
                    println!(
                        "[StreamingToolCalls] Failed to parse arguments for {}: {}",
                        name, e
                    );
                    println!("[StreamingToolCalls] Raw arguments: {}", arguments_str);
                    Value::Object(serde_json::Map::new())
                })
            };

            // Parse the tool name (may be "server___tool" format)
            let (server, tool) = parse_combined_tool_name(&name);

            // Build raw representation for display
            let raw = format!(
                "<tool_call>{{\"name\": \"{}\", \"arguments\": {}}}</tool_call>",
                name,
                serde_json::to_string(&arguments).unwrap_or_else(|_| "{}".to_string())
            );

            // Include the native tool call ID if present
            let id = if tool_call_id.is_empty() {
                None
            } else {
                Some(tool_call_id)
            };

            result.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw,
                id,
            });
        }

        result
    }
}

/// Extract streamed text from either Chat Completions or Responses API payloads.
pub fn extract_text_from_stream_chunk(json: &Value, use_responses_api: bool) -> Option<String> {
    // Chat Completions delta string form
    if let Some(content) = json
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
    {
        if let Some(text) = content.as_str() {
            if !text.is_empty() {
                return Some(text.to_string());
            }
        } else if let Some(parts) = content.as_array() {
            let mut buf = String::new();
            for part in parts {
                if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                    buf.push_str(text);
                } else if let Some(text) = part.as_str() {
                    buf.push_str(text);
                }
            }
            if !buf.is_empty() {
                return Some(buf);
            }
        }
    }

    if use_responses_api {
        // Responses API event shapes (best-effort)
        let candidates = [
            json.get("output_text_delta")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            json.pointer("/delta/output_text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            json.pointer("/response/output_text")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ];

        for cand in candidates {
            if let Some(text) = cand {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }

        if let Some(delta_obj) = json.get("delta") {
            if let Some(text) = delta_obj
                .get("output_text_delta")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(text.to_string());
            }

            if let Some(output_arr) = delta_obj.get("output").and_then(|v| v.as_array()) {
                let mut buf = String::new();
                for entry in output_arr {
                    if let Some(content_arr) = entry.get("content").and_then(|c| c.as_array()) {
                        for part in content_arr {
                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                buf.push_str(text);
                            }
                        }
                    }
                }
                if !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_text_from_stream_chunk_handles_chat_delta_string() {
        let payload = json!({"choices":[{"delta":{"content":"hello"}}]});
        let extracted = extract_text_from_stream_chunk(&payload, false);
        assert_eq!(extracted.as_deref(), Some("hello"));
    }

    #[test]
    fn extract_text_from_stream_chunk_handles_responses_delta() {
        let payload = json!({"type":"response.output_text.delta","output_text_delta":"hello-resp"});
        let extracted = extract_text_from_stream_chunk(&payload, true);
        assert_eq!(extracted.as_deref(), Some("hello-resp"));
    }
}
