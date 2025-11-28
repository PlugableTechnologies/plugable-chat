//! Tool Format Adapters
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
//! - Gemini: function_call in response, "function" role for results  
//! - Granite: <function_call>XML</function_call> format

use serde_json::{json, Value};
use regex::Regex;
use crate::protocol::{ModelFamily, ToolFormat, OpenAITool, ParsedToolCall};

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
            let function_declarations: Vec<Value> = tools.iter().map(|t| {
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
            }).collect();
            
            json!([{
                "function_declarations": function_declarations
            }])
        }
        ToolFormat::Granite => {
            // Granite uses a similar format to OpenAI but may need schema adjustments
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
pub fn parse_tool_calls_for_model(
    response: &str,
    _family: ModelFamily,
    tool_format: ToolFormat,
) -> Vec<ParsedToolCall> {
    match tool_format {
        ToolFormat::OpenAI => {
            // OpenAI format uses structured tool_calls in the response JSON
            // This is typically handled at the streaming level, not from text
            // Fall back to Hermes parser for text-based detection
            parse_hermes_tool_calls(response)
        }
        ToolFormat::Hermes => {
            parse_hermes_tool_calls(response)
        }
        ToolFormat::Gemini => {
            parse_gemini_tool_calls(response)
        }
        ToolFormat::Granite => {
            parse_granite_tool_calls(response)
        }
        ToolFormat::TextBased => {
            // For text-based, we use the generic XML-style parser
            parse_hermes_tool_calls(response)
        }
    }
}

/// Parse Hermes-style tool calls: <tool_call>{"name": "...", "arguments": {...}}</tool_call>
/// This is used by Phi, Qwen, and as a fallback for other formats.
fn parse_hermes_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    
    // Match <tool_call> with optional whitespace
    let re = Regex::new(r"(?s)<tool_call>\s*(.*?)\s*</tool_call>").unwrap();
    
    // Also check for unclosed tool calls (streaming)
    let unclosed_re = Regex::new(r"(?s)<tool_call>\s*(\{.*)").ok();
    
    for cap in re.captures_iter(content) {
        if let Some(json_match) = cap.get(1) {
            let json_str = json_match.as_str().trim();
            let fixed_json = fix_llm_json(json_str);
            
            if let Ok(parsed) = serde_json::from_str::<Value>(&fixed_json) {
                let raw = cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default();
                
                // Try Format 1: {"server": "...", "tool": "...", "arguments": {...}}
                if let (Some(server), Some(tool)) = (
                    parsed.get("server").and_then(|v| v.as_str()),
                    parsed.get("tool").and_then(|v| v.as_str()),
                ) {
                    let arguments = parsed.get("arguments")
                        .cloned()
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    
                    calls.push(ParsedToolCall {
                        server: server.to_string(),
                        tool: tool.to_string(),
                        arguments,
                        raw,
                    });
                    continue;
                }
                
                // Try Format 2: {"name": "server___tool", "arguments": {...}}
                if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                    let arguments = parsed.get("arguments")
                        .cloned()
                        .unwrap_or(Value::Object(serde_json::Map::new()));
                    
                    let (server, tool) = parse_combined_tool_name(name);
                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw,
                    });
                }
            }
        }
    }
    
    // If no tool calls found, check for unclosed tool calls (streaming)
    if calls.is_empty() {
        if let Some(unclosed_re) = unclosed_re {
            if let Some(cap) = unclosed_re.captures(content) {
                if let Some(json_match) = cap.get(1) {
                    let json_str = json_match.as_str().trim();
                    
                    if let Some(balanced_json) = extract_balanced_braces(json_str) {
                        let fixed_json = fix_llm_json(&balanced_json);
                        
                        if let Ok(parsed) = serde_json::from_str::<Value>(&fixed_json) {
                            if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                                let arguments = parsed.get("arguments")
                                    .cloned()
                                    .unwrap_or(Value::Object(serde_json::Map::new()));
                                
                                let (server, tool) = parse_combined_tool_name(name);
                                calls.push(ParsedToolCall {
                                    server,
                                    tool,
                                    arguments,
                                    raw: format!("<tool_call>{}</tool_call>", balanced_json),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    
    calls
}

/// Parse Gemini function_call format
fn parse_gemini_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    
    // Gemini may output function calls in JSON format
    // Try to find function_call patterns
    let re = Regex::new(r#"(?s)"function_call"\s*:\s*\{(.*?)\}"#).ok();
    
    if let Some(re) = re {
        for cap in re.captures_iter(content) {
            if let Some(inner) = cap.get(1) {
                let json_str = format!("{{{}}}", inner.as_str());
                if let Ok(parsed) = serde_json::from_str::<Value>(&json_str) {
                    if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                        let arguments = parsed.get("args")
                            .or_else(|| parsed.get("arguments"))
                            .cloned()
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        
                        let (server, tool) = parse_combined_tool_name(name);
                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                        });
                    }
                }
            }
        }
    }
    
    // Fallback to Hermes parser if no Gemini-style calls found
    if calls.is_empty() {
        return parse_hermes_tool_calls(content);
    }
    
    calls
}

/// Parse Granite <function_call> format
fn parse_granite_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    
    // Granite uses <function_call> tags
    let re = Regex::new(r"(?s)<function_call>\s*(.*?)\s*</function_call>").ok();
    
    if let Some(re) = re {
        for cap in re.captures_iter(content) {
            if let Some(inner) = cap.get(1) {
                let call_content = inner.as_str().trim();
                
                // Try parsing as JSON first
                if let Ok(parsed) = serde_json::from_str::<Value>(call_content) {
                    if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
                        let arguments = parsed.get("arguments")
                            .or_else(|| parsed.get("parameters"))
                            .cloned()
                            .unwrap_or(Value::Object(serde_json::Map::new()));
                        
                        let (server, tool) = parse_combined_tool_name(name);
                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                        });
                    }
                } else {
                    // Try XML-style parsing: <name>...</name><arguments>...</arguments>
                    let name_re = Regex::new(r"<name>(.*?)</name>").ok();
                    let args_re = Regex::new(r"(?s)<arguments>(.*?)</arguments>").ok();
                    
                    if let (Some(name_re), Some(args_re)) = (name_re, args_re) {
                        if let Some(name_cap) = name_re.captures(call_content) {
                            let name = name_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                            let arguments = args_re.captures(call_content)
                                .and_then(|c| c.get(1))
                                .and_then(|m| serde_json::from_str::<Value>(m.as_str()).ok())
                                .unwrap_or(Value::Object(serde_json::Map::new()));
                            
                            let (server, tool) = parse_combined_tool_name(name);
                            calls.push(ParsedToolCall {
                                server,
                                tool,
                                arguments,
                                raw: cap.get(0).map(|m| m.as_str().to_string()).unwrap_or_default(),
                            });
                        }
                    }
                }
            }
        }
    }
    
    // Fallback to Hermes parser if no Granite-style calls found
    if calls.is_empty() {
        return parse_hermes_tool_calls(content);
    }
    
    calls
}

/// Format a tool result for injection into the chat history based on model format
pub fn format_tool_result(
    call: &ParsedToolCall,
    result: &str,
    is_error: bool,
    tool_format: ToolFormat,
) -> String {
    match tool_format {
        ToolFormat::OpenAI => {
            // OpenAI format - this would typically be a separate message with role "tool"
            // For text-based injection, we use a simple format
            if is_error {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\" error=\"true\">\n{}\n</tool_result>",
                    call.server, call.tool, result
                )
            } else {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\">\n{}\n</tool_result>",
                    call.server, call.tool, result
                )
            }
        }
        ToolFormat::Hermes => {
            // Hermes models expect results in a similar XML format
            if is_error {
                format!(
                    "<tool_response error=\"true\">\n{}\n</tool_response>",
                    result
                )
            } else {
                format!(
                    "<tool_response>\n{}\n</tool_response>",
                    result
                )
            }
        }
        ToolFormat::Gemini => {
            // Gemini uses function_response format
            format!(
                "{{\"function_response\": {{\"name\": \"{}___{}\", \"response\": {}}}}}",
                call.server,
                call.tool,
                if is_error {
                    format!("{{\"error\": \"{}\"}}", result.replace('"', "\\\""))
                } else {
                    format!("{{\"result\": \"{}\"}}", result.replace('"', "\\\""))
                }
            )
        }
        ToolFormat::Granite => {
            // Granite uses <function_response> tags
            if is_error {
                format!(
                    "<function_response error=\"true\">\n{}\n</function_response>",
                    result
                )
            } else {
                format!(
                    "<function_response>\n{}\n</function_response>",
                    result
                )
            }
        }
        ToolFormat::TextBased => {
            // Generic text-based format
            if is_error {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\" error=\"true\">\n{}\n</tool_result>\n\n\
                    **TOOL ERROR**: The tool call failed. Please analyze the error and try again with corrected parameters.",
                    call.server, call.tool, result
                )
            } else {
                format!(
                    "<tool_result server=\"{}\" tool=\"{}\">\n{}\n</tool_result>",
                    call.server, call.tool, result
                )
            }
        }
    }
}

// Helper functions

/// Fix common JSON issues from LLMs (trailing commas, etc.)
fn fix_llm_json(json_str: &str) -> String {
    let mut result = json_str.to_string();
    let trailing_comma_re = Regex::new(r",(\s*[}\]])").unwrap();
    result = trailing_comma_re.replace_all(&result, "$1").to_string();
    result
}

/// Parse a combined "server___tool" name into (server, tool)
fn parse_combined_tool_name(combined: &str) -> (String, String) {
    let parts: Vec<&str> = combined.splitn(2, "___").collect();
    if parts.len() == 2 {
        (parts[0].to_string(), parts[1].to_string())
    } else {
        ("unknown".to_string(), combined.to_string())
    }
}

/// Extract a balanced {} block from the start of a string
fn extract_balanced_braces(s: &str) -> Option<String> {
    if !s.starts_with('{') {
        return None;
    }
    
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;
    
    for (i, c) in s.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        
        match c {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(s[..=i].to_string());
                }
            }
            _ => {}
        }
    }
    
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    
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
    fn test_parse_granite_tool_call() {
        let content = r#"Let me call the function.
<function_call>{"name": "mcp___read_file", "arguments": {"path": "/tmp/test.txt"}}</function_call>"#;
        
        let calls = parse_granite_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].server, "mcp");
        assert_eq!(calls[0].tool, "read_file");
    }
    
    #[test]
    fn test_format_tool_result_hermes() {
        let call = ParsedToolCall {
            server: "test".to_string(),
            tool: "echo".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
        };
        
        let result = format_tool_result(&call, "Hello, World!", false, ToolFormat::Hermes);
        assert!(result.contains("<tool_response>"));
        assert!(result.contains("Hello, World!"));
    }
}

