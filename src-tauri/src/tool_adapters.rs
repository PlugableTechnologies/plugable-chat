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

use crate::protocol::{ModelFamily, OpenAITool, ParsedToolCall, ToolFormat};
use crate::settings::{ToolCallFormatConfig, ToolCallFormatName};
use regex::Regex;
use serde_json::{json, Value};

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
            ToolCallFormatName::Hermes => parse_hermes_tool_calls(response),
            ToolCallFormatName::Mistral => parse_tagged_tool_calls(response),
            ToolCallFormatName::Pythonic => parse_pythonic_tool_calls(response),
            ToolCallFormatName::PureJson => parse_pure_json_tool_calls(response),
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
                parse_hermes_tool_calls(response)
            } else {
                Vec::new()
            }
        }
        ToolFormat::Gemini => {
            if formats.is_enabled(ToolCallFormatName::Hermes)
                || formats.is_enabled(ToolCallFormatName::PureJson)
            {
                parse_gemini_tool_calls(response)
            } else {
                Vec::new()
            }
        }
        ToolFormat::Granite | ToolFormat::TextBased => {
            if formats.is_enabled(ToolCallFormatName::Mistral)
                || formats.is_enabled(ToolCallFormatName::Hermes)
            {
                parse_granite_tool_calls(response)
            } else {
                Vec::new()
            }
        }
    }
}

/// Parse Hermes-style tool calls: <tool_call>{"name": "...", "arguments": {...}}</tool_call>
/// This is used by Phi, Qwen, and as a fallback for other formats.
/// Also handles:
/// - markdown code blocks (for smaller models that ignore instructions)
/// - `tool_name`/`tool_args` format (GPT-OSS legacy)
/// - `parameters` as alias for `arguments` (Llama)
/// - `<function=name>{...}</function>` format (Braintrust Llama recipe)
/// - Case-insensitive tags (<Tool_Call>, <TOOL_CALL>)
/// - Tags with attributes (<tool_call id="1">)
/// - Common typos (<toolcall>, <tool-call>, <tool_calls>)
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
            let fixed_json = fix_llm_json(json_str);

            if let Some(parsed) = parse_flexible_json(&fixed_json) {
                let raw = cap
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                // Try Format 1: {"server": "...", "tool": "...", "arguments": {...}}
                if let (Some(server), Some(tool)) = (
                    parsed.get("server").and_then(|v| v.as_str()),
                    parsed.get("tool").and_then(|v| v.as_str()),
                ) {
                    let arguments = extract_arguments(&parsed);

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
                if let Some(name) = extract_tool_name(&parsed) {
                    let arguments = extract_arguments(&parsed);

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

                    if let Some(balanced_json) = extract_balanced_braces(json_str) {
                        let fixed_json = fix_llm_json(&balanced_json);

                        if let Some(parsed) = parse_flexible_json(&fixed_json) {
                            if let Some(name) = extract_tool_name(&parsed) {
                                let arguments = extract_arguments(&parsed);

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
        for json_str in find_json_objects(content) {
            if let Some(parsed) = parse_flexible_json(&json_str) {
                if let Some(name) = extract_tool_name(&parsed) {
                    let arguments = extract_arguments(&parsed);
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

/// Parse Braintrust-style function calls: <function=get_weather>{"location": "Tokyo"}</function>
/// This format is used by some Llama 3.x recipes
fn parse_braintrust_function_calls(content: &str) -> Vec<ParsedToolCall> {
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

            let fixed_json = fix_llm_json(json_str);
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

/// Parse tool calls from markdown JSON code blocks.
/// Handles formats like:
/// ```json
/// {"name": "tool_name", "arguments": {...}}
/// {"tool_name": "...", "tool_args": {...}}  // GPT-OSS
/// {"name": "...", "parameters": {...}}       // Llama
/// ```
/// This is a fallback for smaller models that ignore <tool_call> format instructions.
fn parse_markdown_json_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match markdown code blocks: ```json ... ``` or ``` ... ```
    // (?s) for DOTALL mode, optional language specifier (json, etc.)
    let code_block_re = Regex::new(r"(?s)```(?:json)?\s*\n?(.*?)\n?```").unwrap();

    for cap in code_block_re.captures_iter(content) {
        if let Some(json_match) = cap.get(1) {
            let json_str = json_match.as_str().trim();

            // Skip if it doesn't look like a tool call JSON (must have name-like field)
            if !json_str.contains("\"name\"") && !json_str.contains("\"tool_name\"") {
                continue;
            }

            let fixed_json = fix_llm_json(json_str);

            if let Ok(parsed) = serde_json::from_str::<Value>(&fixed_json) {
                let raw = cap
                    .get(0)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default();

                // Check if this looks like a tool call (has name-like field)
                if let Some(name) = extract_tool_name(&parsed) {
                    // Skip if it doesn't look like a tool name (e.g., just random JSON)
                    // Tool names should be simple identifiers, not long strings
                    if name.len() > 100 || name.contains('\n') {
                        continue;
                    }

                    let arguments = extract_arguments(&parsed);
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

/// Parse Pythonic function calls inside markdown code blocks.
/// Handles formats like:
/// ```plaintext
/// sql_select("SELECT ...", ["source_id"])
/// ```
/// or similar code blocks with any language tag (python, text, etc.)
fn parse_pythonic_code_block_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Match markdown code blocks with any language or no language
    // (?s) for DOTALL mode
    let code_block_re = Regex::new(r"(?s)```(?:\w*)?\s*\n?(.*?)\n?```").unwrap();

    for cap in code_block_re.captures_iter(content) {
        if let Some(block_content) = cap.get(1) {
            let block_text = block_content.as_str().trim();

            // Try to parse as a Pythonic function call: tool_name(args...)
            // This regex captures the function name and everything inside the parentheses
            // It handles nested parentheses and quoted strings with parentheses
            let pythonic_re = Regex::new(r"^([A-Za-z_][A-Za-z0-9_]*)\s*\(([\s\S]*)\)\s*$").ok();

            if let Some(pythonic_re) = pythonic_re {
                if let Some(func_cap) = pythonic_re.captures(block_text) {
                    let name = func_cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
                    let args_str = func_cap.get(2).map(|m| m.as_str()).unwrap_or("");

                    if name.is_empty() {
                        continue;
                    }

                    // Known tool names to filter out false positives
                    let known_tools = [
                        "sql_select",
                        "schema_search",
                        "tool_search",
                        "python_execution",
                    ];

                    // Only parse if it looks like a known tool or has server prefix
                    let is_known = known_tools.contains(&name) || name.contains("___");

                    if !is_known {
                        // Skip unknown function names to avoid false positives
                        continue;
                    }

                    // Use tool-specific positional argument parsing for sql_select
                    let arguments = if name == "sql_select" {
                        parse_sql_select_positional_arguments(args_str)
                    } else {
                        parse_pythonic_arguments(args_str)
                    };
                    let (server, tool) = parse_combined_tool_name(name);

                    let raw = cap
                        .get(0)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_default();

                    println!(
                        "[parse_pythonic_code_block_tool_calls] Found tool call in code block: {} (server: {})",
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
        }
    }

    calls
}

/// Parse positional arguments for sql_select tool.
/// Handles formats like:
/// - sql_select("SELECT ...", ["source_id"]) - positional with array
/// - sql_select("SELECT ...") - just the SQL query
/// - sql_select(sql="SELECT ...", source_id="x") - named arguments
fn parse_sql_select_positional_arguments(args_str: &str) -> Value {
    let trimmed = args_str.trim();
    
    // If it looks like named arguments (contains `sql=` or `source_id=`), use standard parser
    if trimmed.contains("sql=") || trimmed.contains("source_id=") {
        return parse_pythonic_arguments(args_str);
    }
    
    // Try to extract positional arguments: first is SQL string, second is optional source_id array/string
    let positional = extract_positional_arguments(trimmed);
    
    if positional.is_empty() {
        return Value::Object(serde_json::Map::new());
    }
    
    let mut map = serde_json::Map::new();
    
    // First positional argument is the SQL query
    if let Some(sql_val) = positional.get(0) {
        match sql_val {
            Value::String(s) => {
                map.insert("sql".to_string(), Value::String(s.clone()));
            }
            _ => {
                // If it's not a string, try to convert it
                if let Some(s) = sql_val.as_str() {
                    map.insert("sql".to_string(), Value::String(s.to_string()));
                }
            }
        }
    }
    
    // Second positional argument is the source_id (array or string)
    if let Some(source_val) = positional.get(1) {
        match source_val {
            Value::Array(arr) => {
                // If it's an array, take the first element as source_id
                if let Some(first) = arr.first() {
                    if let Some(s) = first.as_str() {
                        map.insert("source_id".to_string(), Value::String(s.to_string()));
                    }
                }
            }
            Value::String(s) => {
                map.insert("source_id".to_string(), Value::String(s.clone()));
            }
            _ => {}
        }
    }
    
    Value::Object(map)
}

/// Extract positional arguments from a function call, handling quoted strings and nested structures.
fn extract_positional_arguments(args_str: &str) -> Vec<Value> {
    let mut arguments: Vec<Value> = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut string_char = '\0';
    let mut depth = 0;  // Track nesting of [], {}
    let mut escape_next = false;
    
    for ch in args_str.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }
        
        match ch {
            '\\' if in_string => {
                current.push(ch);
                escape_next = true;
            }
            '"' | '\'' => {
                if !in_string {
                    in_string = true;
                    string_char = ch;
                } else if ch == string_char {
                    in_string = false;
                }
                current.push(ch);
            }
            '[' | '{' if !in_string => {
                depth += 1;
                current.push(ch);
            }
            ']' | '}' if !in_string => {
                depth -= 1;
                current.push(ch);
            }
            ',' if !in_string && depth == 0 => {
                // End of argument
                let arg = current.trim().to_string();
                if !arg.is_empty() {
                    arguments.push(parse_positional_value(&arg));
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    
    // Don't forget the last argument
    let arg = current.trim().to_string();
    if !arg.is_empty() {
        arguments.push(parse_positional_value(&arg));
    }
    
    arguments
}

/// Parse a single positional value (string, array, etc.)
fn parse_positional_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    
    // Quoted string
    if (trimmed.starts_with('"') && trimmed.ends_with('"'))
        || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        // Handle escape sequences
        let unescaped = inner
            .replace("\\\"", "\"")
            .replace("\\'", "'")
            .replace("\\n", "\n")
            .replace("\\t", "\t")
            .replace("\\\\", "\\");
        return Value::String(unescaped);
    }
    
    // Try parsing as JSON (for arrays, objects, numbers, booleans)
    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        return val;
    }
    
    // Fallback to string
    Value::String(trimmed.to_string())
}

/// Parse Pythonic function-call style tool invocations: `tool_name(arg="value")`
fn parse_pythonic_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let re = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)").unwrap();

    for cap in re.captures_iter(content) {
        let name = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let args_str = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        // Use tool-specific positional argument parsing for sql_select
        let arguments = if name == "sql_select" {
            parse_sql_select_positional_arguments(args_str)
        } else {
            parse_pythonic_arguments(args_str)
        };
        let (server, tool) = parse_combined_tool_name(name);
        calls.push(ParsedToolCall {
            server,
            tool,
            arguments,
            raw: cap
                .get(0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            id: None,
        });
    }

    calls
}

fn parse_pythonic_arguments(arg_str: &str) -> Value {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut quote_char = '\0';
    let mut paren_depth = 0;

    for ch in arg_str.chars() {
        match ch {
            '"' | '\'' => {
                if in_string && ch == quote_char {
                    in_string = false;
                } else if !in_string {
                    in_string = true;
                    quote_char = ch;
                }
                current.push(ch);
            }
            '(' | '[' | '{' => {
                if !in_string {
                    paren_depth += 1;
                }
                current.push(ch);
            }
            ')' | ']' | '}' => {
                if !in_string && paren_depth > 0 {
                    paren_depth -= 1;
                }
                current.push(ch);
            }
            ',' if !in_string && paren_depth == 0 => {
                if !current.trim().is_empty() {
                    parts.push(current.trim().to_string());
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }

    let mut map = serde_json::Map::new();
    for part in parts {
        if let Some((k, v)) = part.split_once('=') {
            let key = k.trim();
            if key.is_empty() {
                continue;
            }
            let value = parse_pythonic_value(v.trim());
            map.insert(key.to_string(), value);
        }
    }

    Value::Object(map)
}

fn parse_pythonic_value(raw: &str) -> Value {
    let trimmed = raw.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"')
        || trimmed.starts_with('\'') && trimmed.ends_with('\'')
    {
        return Value::String(trimmed.trim_matches(|c| c == '"' || c == '\'').to_string());
    }

    match trimmed.to_ascii_lowercase().as_str() {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" | "none" => Value::Null,
        _ => {
            if let Ok(n) = trimmed.parse::<i64>() {
                Value::Number(n.into())
            } else if let Ok(f) = trimmed.parse::<f64>() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or_else(|| Value::String(trimmed.to_string()))
            } else {
                Value::String(trimmed.to_string())
            }
        }
    }
}

/// Parse pure JSON object/array tool calls without tags.
fn parse_pure_json_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return calls;
    }

    if let Some(value) = parse_flexible_json(trimmed) {
        collect_calls_from_value(&value, trimmed, &mut calls);
    }

    if calls.is_empty() {
        calls = parse_markdown_json_tool_calls(content);
    }

    calls
}

fn collect_calls_from_value(value: &Value, raw: &str, calls: &mut Vec<ParsedToolCall>) {
    let entries: Vec<Value> = match value {
        Value::Array(arr) => arr.clone(),
        other => vec![other.clone()],
    };

    for entry in entries {
        // Handle {"tool": "...", "args": {...}}
        if let (Some(tool), Some(args)) = (
            entry.get("tool").and_then(|v| v.as_str()),
            entry.get("args"),
        ) {
            let (server, tool_name) = parse_combined_tool_name(tool);
            calls.push(ParsedToolCall {
                server,
                tool: tool_name,
                arguments: args.clone(),
                raw: raw.to_string(),
                id: None,
            });
            continue;
        }

        if let Some(name) = extract_tool_name(&entry) {
            let arguments = extract_arguments(&entry);
            let (server, tool) = parse_combined_tool_name(&name);
            calls.push(ParsedToolCall {
                server,
                tool,
                arguments,
                raw: raw.to_string(),
                id: None,
            });
        }
    }
}

/// Parse tag-based tool calls such as `[TOOL_CALLS] [{...}]` (Mistral-style).
fn parse_tagged_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let marker = "[TOOL_CALLS]";

    if let Some(idx) = content.find(marker) {
        let mut payload = content[idx + marker.len()..].trim_start();

        // Trim at closing markers if present
        for end_marker in ["[/TOOL_CALLS]", "[TOOL_RESULTS]"] {
            if let Some(pos) = payload.find(end_marker) {
                payload = &payload[..pos];
                break;
            }
        }

        let trimmed = payload.trim();

        // Attempt parsing as-is, then try without surrounding [] if present
        let parsed = parse_flexible_json(trimmed).or_else(|| {
            let without_brackets = trimmed.trim_matches(|c| c == '[' || c == ']');
            parse_flexible_json(without_brackets)
        });

        if let Some(value) = parsed {
            let entries = match value {
                Value::Array(arr) => arr,
                other => vec![other],
            };

            for entry in entries {
                if let Some(name) = extract_tool_name(&entry) {
                    let arguments = extract_arguments(&entry);
                    let (server, tool) = parse_combined_tool_name(&name);

                    calls.push(ParsedToolCall {
                        server,
                        tool,
                        arguments,
                        raw: trimmed.to_string(),
                        id: None,
                    });
                }
            }
        }
    }

    calls
}

/// Parse Gemini function_call format
pub fn parse_gemini_tool_calls(content: &str) -> Vec<ParsedToolCall> {
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
                        let arguments = parsed
                            .get("args")
                            .or_else(|| parsed.get("arguments"))
                            .cloned()
                            .unwrap_or(Value::Object(serde_json::Map::new()));

                        let (server, tool) = parse_combined_tool_name(name);
                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap
                                .get(0)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                            id: None,
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

/// Parse Granite <function_call> format (also used by Gemma)
/// Handles JSON inside <function_call> tags with flexible field names
pub fn parse_granite_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();

    // Granite/Gemma uses <function_call> tags
    let re = Regex::new(r"(?s)<function_call>\s*(.*?)\s*</function_call>").ok();

    if let Some(re) = re {
        for cap in re.captures_iter(content) {
            if let Some(inner) = cap.get(1) {
                let call_content = inner.as_str().trim();

                // Try parsing as JSON first
                let fixed_json = fix_llm_json(call_content);
                if let Ok(parsed) = serde_json::from_str::<Value>(&fixed_json) {
                    // Use helper functions for flexible field name extraction
                    if let Some(name) = extract_tool_name(&parsed) {
                        let arguments = extract_arguments(&parsed);
                        let (server, tool) = parse_combined_tool_name(&name);

                        calls.push(ParsedToolCall {
                            server,
                            tool,
                            arguments,
                            raw: cap
                                .get(0)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                            id: None,
                        });
                    }
                } else {
                    // Try XML-style parsing: <name>...</name><arguments>...</arguments>
                    let name_re = Regex::new(r"<name>(.*?)</name>").ok();
                    let args_re = Regex::new(r"(?s)<arguments>(.*?)</arguments>").ok();

                    if let (Some(name_re), Some(args_re)) = (name_re, args_re) {
                        if let Some(name_cap) = name_re.captures(call_content) {
                            let name = name_cap.get(1).map(|m| m.as_str()).unwrap_or("");
                            let arguments = args_re
                                .captures(call_content)
                                .and_then(|c| c.get(1))
                                .and_then(|m| serde_json::from_str::<Value>(m.as_str()).ok())
                                .unwrap_or(Value::Object(serde_json::Map::new()));

                            let (server, tool) = parse_combined_tool_name(name);
                            calls.push(ParsedToolCall {
                                server,
                                tool,
                                arguments,
                                raw: cap
                                    .get(0)
                                    .map(|m| m.as_str().to_string())
                                    .unwrap_or_default(),
                                id: None,
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

/// Success guidance for sql_select - tells model that results have been shown to user
const SQL_SUCCESS_GUIDANCE: &str = "\n\n**NOTE**: The query results above have already been displayed to the user in a formatted table. \
Your role now is to provide helpful commentary: summarize key insights, suggest follow-up analyses, \
or answer any specific questions the user may have about the data. Do NOT repeat the raw data.";

/// Build error guidance string, optionally including the original user prompt
fn build_error_guidance(tool_name: &str, original_user_prompt: Option<&str>) -> String {
    let base_guidance = if tool_name == "sql_select" {
        "**SQL ERROR - RETRY REQUIRED**: The query failed. You MUST retry (up to 3 attempts).\n\n\
        **STEP 1 - Identify the Error**:\n\
        Read the error message above carefully. Common issues:\n\
        - \"Unrecognized name\" = You used a column that doesn't exist. Check the EXACT column names in the schema.\n\
        - \"Function not found\" = Use database-appropriate functions (use CAST(column AS STRING), not TO_CHAR)\n\
        - Syntax error = Check SQL dialect compatibility\n\n\
        **STEP 2 - Review the Schema**:\n\
        Go back to the 'Database Context' section in this prompt. Look at the 'Columns:' list.\n\
        ONLY use columns that are EXPLICITLY listed there. Do NOT invent or guess column names.\n\n\
        **STEP 3 - Retry with Corrected SQL**:\n\
        Make the fix and try again immediately. Do NOT give up or tell the user you can't help.\n\
        You have tools available - USE THEM."
    } else {
        "**TOOL ERROR - RETRY REQUIRED**: The tool call failed. You MUST retry (up to 3 attempts).\n\n\
        **STEP 1**: Read the error message carefully to understand what went wrong.\n\
        **STEP 2**: Review the tool schema for correct parameter names and types.\n\
        **STEP 3**: Retry with corrected parameters immediately.\n\n\
        Do NOT give up or tell the user you cannot help. You have the tools - USE THEM."
    };

    match original_user_prompt {
        Some(prompt) if !prompt.is_empty() => {
            format!(
                "\n\n{}\n\n**REMINDER - Original User Request**: \"{}\"\n\n⚠️ TRY AGAIN NOW with a corrected tool call.",
                base_guidance, prompt
            )
        }
        _ => format!("\n\n{}\n\n⚠️ TRY AGAIN NOW with a corrected tool call.", base_guidance),
    }
}

/// Format a tool result for injection into the chat history based on model format
/// 
/// When `is_error` is true and `original_user_prompt` is provided, the error guidance
/// will include a reminder of what the user originally asked, helping the model
/// understand the context for its retry.
pub fn format_tool_result(
    call: &ParsedToolCall,
    result: &str,
    is_error: bool,
    tool_format: ToolFormat,
    original_user_prompt: Option<&str>,
) -> String {
    let guidance = if is_error {
        build_error_guidance(&call.tool, original_user_prompt)
    } else if call.tool == "sql_select" {
        SQL_SUCCESS_GUIDANCE.to_string()
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

// Helper functions

/// Fix common JSON issues from LLMs.
/// Handles: trailing commas, Python booleans/None, BOM, comments, unescaped newlines.
fn fix_llm_json(json_str: &str) -> String {
    // Strip BOM and invisible Unicode characters
    let mut result = json_str
        .trim_start_matches('\u{feff}') // UTF-8 BOM
        .trim_start_matches('\u{fffe}') // UTF-16 BOM marker
        .to_string();

    // Remove line comments (// ...) - must be careful not to match inside strings
    // Simple approach: only remove if // is at line start or after whitespace
    if let Ok(line_comment_re) = Regex::new(r"(?m)^\s*//.*$") {
        result = line_comment_re.replace_all(&result, "").to_string();
    }

    // Remove block comments (/* ... */)
    if let Ok(block_comment_re) = Regex::new(r"(?s)/\*.*?\*/") {
        result = block_comment_re.replace_all(&result, "").to_string();
    }

    // Replace Python booleans with JSON booleans
    // Use word boundaries to avoid replacing inside strings (best effort)
    if let Ok(true_re) = Regex::new(r"\bTrue\b") {
        result = true_re.replace_all(&result, "true").to_string();
    }
    if let Ok(false_re) = Regex::new(r"\bFalse\b") {
        result = false_re.replace_all(&result, "false").to_string();
    }

    // Replace Python None with JSON null
    if let Ok(none_re) = Regex::new(r"\bNone\b") {
        result = none_re.replace_all(&result, "null").to_string();
    }

    // Fix trailing commas before } or ]
    if let Ok(trailing_comma_re) = Regex::new(r",(\s*[}\]])") {
        result = trailing_comma_re.replace_all(&result, "$1").to_string();
    }

    // Fix unescaped literal newlines inside strings (replace with \n)
    // This is tricky - we'll do a simple fix for obvious cases
    if let Ok(newline_in_string_re) = Regex::new(r#"("(?:[^"\\]|\\.)*)\n((?:[^"\\]|\\.)*")"#) {
        // Apply multiple times in case of multiple newlines
        for _ in 0..5 {
            let new_result = newline_in_string_re.replace_all(&result, "$1\\n$2").to_string();
            if new_result == result {
                break;
            }
            result = new_result;
        }
    }

    result
}

/// Parse JSON with lenient fallbacks.
/// Fallback chain:
/// 1. Direct serde_json parse (fast path)
/// 2. fix_llm_json preprocessing + serde_json
/// 3. Single quote replacement + serde_json
/// 4. json5 parser (handles unquoted keys, comments, trailing commas)
/// 5. Balanced brace extraction + retry
fn parse_flexible_json(raw: &str) -> Option<Value> {
    // Fast path: try direct parse
    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        return Some(unwrap_structure(val));
    }

    // Fix trivial JSON issues first
    let fixed = fix_llm_json(raw);
    if let Ok(val) = serde_json::from_str::<Value>(&fixed) {
        return Some(unwrap_structure(val));
    }

    // Fallback: try replacing single quotes with double quotes
    let single_to_double = fixed.replace('\'', "\"");
    if let Ok(val) = serde_json::from_str::<Value>(&single_to_double) {
        return Some(unwrap_structure(val));
    }

    // Fallback: try json5 parser (handles unquoted keys, comments, etc.)
    if let Ok(val) = json5::from_str::<Value>(&fixed) {
        return Some(unwrap_structure(val));
    }

    // Fallback: try extracting balanced braces and retry
    if let Some(balanced) = extract_balanced_braces(raw.trim()) {
        if balanced != raw {
            let fixed_balanced = fix_llm_json(&balanced);
            if let Ok(val) = serde_json::from_str::<Value>(&fixed_balanced) {
                return Some(unwrap_structure(val));
            }
            if let Ok(val) = json5::from_str::<Value>(&fixed_balanced) {
                return Some(unwrap_structure(val));
            }
        }
    }

    None
}

/// Unwrap common structural wrappers from parsed JSON.
/// Handles:
/// - Single-element arrays: [{"name": ...}] -> {"name": ...}
/// - Nested wrappers: {"tool_call": {"name": ...}} -> {"name": ...}
fn unwrap_structure(value: Value) -> Value {
    // Unwrap single-element arrays
    if let Value::Array(arr) = &value {
        if arr.len() == 1 {
            return unwrap_structure(arr[0].clone());
        }
    }

    // Unwrap known wrapper keys
    if let Value::Object(map) = &value {
        // Check for wrapper keys that contain the actual tool call
        let wrapper_keys = ["tool_call", "function_call", "call", "tool", "function"];
        for key in wrapper_keys {
            if let Some(inner) = map.get(key) {
                // Only unwrap if the inner value looks like a tool call (has name field)
                if inner.get("name").is_some() || inner.get("tool_name").is_some() {
                    return unwrap_structure(inner.clone());
                }
            }
        }
    }

    value
}

/// Extract tool name from parsed JSON, supporting multiple formats:
/// - `name` (standard)
/// - `tool_name` (GPT-OSS legacy)
/// - `function` (alternative)
/// - `tool` (when it's a string value, not an object)
/// - `action`, `command` (less common aliases)
/// - Nested paths: `tool.name`, `function.name`, `call.name`
fn extract_tool_name(parsed: &Value) -> Option<String> {
    // Direct field aliases (in order of preference)
    let name_fields = ["name", "tool_name", "function", "action", "command"];

    for field in name_fields {
        if let Some(name) = parsed.get(field).and_then(|v| v.as_str()) {
            // Validate it looks like a tool name
            if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                return Some(name.to_string());
            }
        }
    }

    // Check "tool" field - if it's a string, use it as the name
    if let Some(tool_val) = parsed.get("tool") {
        if let Some(name) = tool_val.as_str() {
            if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                return Some(name.to_string());
            }
        }
    }

    // Search nested paths
    let nested_paths = [
        ("tool", "name"),
        ("function", "name"),
        ("call", "name"),
        ("tool_call", "name"),
        ("function_call", "name"),
    ];

    for (outer, inner) in nested_paths {
        if let Some(outer_obj) = parsed.get(outer) {
            if let Some(name) = outer_obj.get(inner).and_then(|v| v.as_str()) {
                if !name.is_empty() && name.len() < 200 && !name.contains('\n') {
                    return Some(name.to_string());
                }
            }
        }
    }

    None
}

/// Extract arguments from parsed JSON, supporting multiple formats:
/// - `arguments` (standard)
/// - `parameters` (Llama format)
/// - `tool_args` (GPT-OSS legacy)
fn extract_arguments(parsed: &Value) -> Value {
    // Try "arguments" first (standard format)
    if let Some(args) = parsed.get("arguments") {
        return args.clone();
    }
    // Try "parameters" (Llama format)
    if let Some(args) = parsed.get("parameters") {
        return args.clone();
    }
    // Try "tool_args" (GPT-OSS legacy format)
    if let Some(args) = parsed.get("tool_args") {
        return args.clone();
    }
    // Default to empty object
    Value::Object(serde_json::Map::new())
}

/// Parse a combined "server___tool" name into (server, tool)
pub fn parse_combined_tool_name(combined: &str) -> (String, String) {
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

/// Find all balanced JSON objects in content.
/// Returns a vector of JSON strings that contain tool-call-like fields.
fn find_json_objects(content: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '{' {
            // Try to extract balanced braces starting from this position
            if let Some(json_str) = extract_balanced_braces(&content[i..]) {
                // Check if it looks like a tool call (has name-like field)
                if json_str.contains("\"name\"")
                    || json_str.contains("\"tool_name\"")
                    || json_str.contains("'name'")
                    || json_str.contains("'tool_name'")
                {
                    objects.push(json_str.clone());
                }
                // Skip past this object
                i += json_str.len();
                continue;
            }
        }
        i += 1;
    }

    objects
}

/// Last-resort extraction: use regex to extract tool name and arguments directly.
/// This handles cases where JSON parsing fails completely.
fn extract_tool_call_by_regex(content: &str) -> Option<ParsedToolCall> {
    // Try to extract the name field
    let name_re = Regex::new(r#"["']?(name|tool_name)["']?\s*:\s*["']([^"']+)["']"#).ok()?;
    let name_cap = name_re.captures(content)?;
    let name = name_cap.get(2)?.as_str().to_string();

    if name.is_empty() || name.len() > 100 || name.contains('\n') {
        return None;
    }

    // Try to extract arguments - look for arguments/parameters/tool_args followed by an object
    let args_re =
        Regex::new(r#"["']?(arguments|parameters|tool_args)["']?\s*:\s*(\{[^{}]*\})"#).ok();
    let arguments = if let Some(args_re) = args_re {
        if let Some(args_cap) = args_re.captures(content) {
            if let Some(args_str) = args_cap.get(2) {
                parse_flexible_json(args_str.as_str()).unwrap_or(Value::Object(serde_json::Map::new()))
            } else {
                Value::Object(serde_json::Map::new())
            }
        } else {
            Value::Object(serde_json::Map::new())
        }
    } else {
        Value::Object(serde_json::Map::new())
    };

    let (server, tool) = parse_combined_tool_name(&name);

    Some(ParsedToolCall {
        server,
        tool,
        arguments,
        raw: content.to_string(),
        id: None,
    })
}

// ============ Python Code Detection ============

/// Detected Python code block from model response
#[derive(Debug, Clone)]
pub struct DetectedPythonCode {
    /// The Python code content (without markdown fence)
    pub code: String,
    /// Start position in original content
    pub start: usize,
    /// End position in original content
    pub end: usize,
    /// Whether this was explicitly marked as Python
    pub explicit_python: bool,
}

/// Detect Python code blocks in model response content.
///
/// Looks for:
/// 1. ```python ... ``` blocks (explicit)
/// 2. ```py ... ``` blocks (explicit, short form)
/// 3. ``` ... ``` blocks that look like Python (implicit)
/// 4. Indented code blocks after "Here's the code:" or similar
///
/// Returns all detected Python code blocks in order of appearance.
pub fn detect_python_code(content: &str) -> Vec<DetectedPythonCode> {
    let mut results = Vec::new();

    // Pattern 1: Explicit ```python or ```py code blocks
    let python_fence_re = Regex::new(r"(?s)```(python|py)\s*\n(.*?)```").unwrap();
    for cap in python_fence_re.captures_iter(content) {
        if let (Some(code_match), Some(full_match)) = (cap.get(2), cap.get(0)) {
            results.push(DetectedPythonCode {
                code: code_match.as_str().trim().to_string(),
                start: full_match.start(),
                end: full_match.end(),
                explicit_python: true,
            });
        }
    }

    // Pattern 2: Generic code blocks that look like Python
    // Only match if not already matched as explicit python
    let generic_fence_re = Regex::new(r"(?s)```\s*\n(.*?)```").unwrap();
    for cap in generic_fence_re.captures_iter(content) {
        if let (Some(code_match), Some(full_match)) = (cap.get(1), cap.get(0)) {
            let code = code_match.as_str();
            let start = full_match.start();

            // Skip if this position is already covered by an explicit python block
            if results.iter().any(|r| start >= r.start && start < r.end) {
                continue;
            }

            // Check if it looks like Python
            if looks_like_python(code) {
                results.push(DetectedPythonCode {
                    code: code.trim().to_string(),
                    start,
                    end: full_match.end(),
                    explicit_python: false,
                });
            }
        }
    }

    // Pattern 3: Indented code after trigger phrases
    // Look for patterns like "Here's the code:\n    import ..."
    let trigger_re = Regex::new(r"(?im)(?:here(?:'s| is) (?:the )?(?:python )?code|execute this|run this):\s*\n((?:[ \t]+[^\n]+\n?)+)").unwrap();
    for cap in trigger_re.captures_iter(content) {
        if let (Some(code_match), Some(full_match)) = (cap.get(1), cap.get(0)) {
            let start = full_match.start();

            // Skip if already matched
            if results.iter().any(|r| start >= r.start && start < r.end) {
                continue;
            }

            // Dedent the code
            let code = dedent_code(code_match.as_str());

            if looks_like_python(&code) {
                results.push(DetectedPythonCode {
                    code,
                    start,
                    end: full_match.end(),
                    explicit_python: false,
                });
            }
        }
    }

    // Sort by position
    results.sort_by_key(|r| r.start);

    results
}

/// Check if code looks like Python based on common patterns
fn looks_like_python(code: &str) -> bool {
    let code_trimmed = code.trim();

    // Empty code doesn't look like Python
    if code_trimmed.is_empty() {
        return false;
    }

    // Strong indicators of Python
    let python_patterns = [
        // Import statements
        r"(?m)^import\s+\w",
        r"(?m)^from\s+\w+\s+import",
        // Function definitions
        r"(?m)^def\s+\w+\s*\(",
        // Class definitions
        r"(?m)^class\s+\w+",
        // Print function
        r"print\s*\(",
        // Python-specific syntax
        r"^\s*if\s+.*:\s*$",
        r"^\s*for\s+\w+\s+in\s+",
        r"^\s*while\s+.*:\s*$",
        r"^\s*elif\s+",
        r"^\s*except\s+",
        r"^\s*with\s+.*:\s*$",
        // List comprehensions
        r"\[.+\s+for\s+\w+\s+in\s+.+\]",
        // f-strings
        r#"f["'][^"']*\{"#,
        // Python comments
        r"(?m)^\s*#[^!]",
    ];

    for pattern in &python_patterns {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(code_trimmed) {
                return true;
            }
        }
    }

    // Negative indicators (not Python)
    let not_python_patterns = [
        r"(?m)^\s*function\s+\w+\s*\(", // JavaScript
        r"(?m)^\s*const\s+\w+\s*=",     // JavaScript/TypeScript
        r"(?m)^\s*let\s+\w+\s*=",       // JavaScript/TypeScript
        r"(?m)^\s*var\s+\w+\s*=",       // JavaScript
        r"(?m)^\s*fn\s+\w+\s*\(",       // Rust
        r"(?m)^\s*impl\s+",             // Rust
        r"(?m)^\s*pub\s+fn\s+",         // Rust
        r"(?m)^\s*int\s+main\s*\(",     // C/C++
        r"(?m)^\s*#include\s*<",        // C/C++
        r"\$\w+",                       // Shell/PHP variables
        r"(?m)^\s*SELECT\s+",           // SQL
    ];

    for pattern in &not_python_patterns {
        if let Ok(re) = Regex::new(pattern) {
            if re.is_match(code_trimmed) {
                return false;
            }
        }
    }

    // If it has assignment with = and no strong Python indicators,
    // check for simple calculations (which might be Python)
    if code_trimmed.contains("=") && code_trimmed.lines().count() <= 5 {
        // Simple variable assignment like "result = 2 + 2"
        if Regex::new(r"^\w+\s*=\s*.+$")
            .map(|re| re.is_match(code_trimmed))
            .unwrap_or(false)
        {
            return true;
        }
    }

    false
}

/// Remove common leading indentation from code
fn dedent_code(code: &str) -> String {
    let lines: Vec<&str> = code.lines().collect();

    // Find minimum indentation (ignoring empty lines)
    let min_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);

    // Remove that indentation from all lines
    lines
        .iter()
        .map(|line| {
            if line.len() >= min_indent {
                &line[min_indent..]
            } else {
                line.trim_start()
            }
        })
        .collect::<Vec<&str>>()
        .join("\n")
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
    fn test_parse_tagged_tool_call_with_single_quotes() {
        let content = "[TOOL_CALLS] [{'name': 'search', 'arguments': {'query': 'AI'}}]";

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "search");
        assert_eq!(
            calls[0].arguments.get("query").and_then(|v| v.as_str()),
            Some("AI")
        );
    }

    #[test]
    fn test_parse_tagged_tool_call_ignores_following_results() {
        let content = "[TOOL_CALLS] [{\"name\": \"calc\", \"arguments\": {\"a\": 1}}][TOOL_RESULTS] placeholder";

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "calc");
        assert_eq!(
            calls[0].arguments.get("a").and_then(|v| v.as_i64()),
            Some(1)
        );
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
    fn test_format_tool_result_hermes() {
        let call = ParsedToolCall {
            server: "test".to_string(),
            tool: "echo".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
            id: None,
        };

        let result = format_tool_result(&call, "Hello, World!", false, ToolFormat::Hermes, None);
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

        // Test all formats include SQL success guidance
        for format in [
            ToolFormat::OpenAI,
            ToolFormat::Hermes,
            ToolFormat::Granite,
            ToolFormat::TextBased,
            ToolFormat::Gemini,
        ] {
            let result = format_tool_result(&call, sql_result, false, format, None);
            assert!(
                result.contains("already been displayed to the user"),
                "Format {:?} should tell model results were shown to user, got: {}",
                format,
                result
            );
            assert!(
                result.contains("provide helpful commentary"),
                "Format {:?} should ask for commentary, got: {}",
                format,
                result
            );
            // Should NOT include error guidance
            assert!(
                !result.contains("TOOL ERROR"),
                "Format {:?} should NOT include error guidance for success, got: {}",
                format,
                result
            );
        }
    }

    #[test]
    fn test_format_tool_result_error_includes_guidance() {
        let call = ParsedToolCall {
            server: "mcp-123".to_string(),
            tool: "search_catalog".to_string(),
            arguments: json!({}),
            raw: "".to_string(),
            id: None,
        };

        let error_msg = "MCP error -32602: provided parameters were invalid: parameter is required";
        let user_prompt = "Show me the catalog items";

        // Test all formats include error guidance with user prompt reminder
        for format in [
            ToolFormat::OpenAI,
            ToolFormat::Hermes,
            ToolFormat::Granite,
            ToolFormat::TextBased,
            ToolFormat::Gemini,
        ] {
            let result = format_tool_result(&call, error_msg, true, format, Some(user_prompt));
            assert!(
                result.contains("TOOL ERROR"),
                "Format {:?} should include error guidance, got: {}",
                format,
                result
            );
            assert!(
                result.contains("MCP error -32602"),
                "Format {:?} should include error code, got: {}",
                format,
                result
            );
            assert!(
                result.contains("RETRY REQUIRED") || result.contains("TRY AGAIN"),
                "Format {:?} should tell model to retry, got: {}",
                format,
                result
            );
            assert!(
                result.contains("Original User Request"),
                "Format {:?} should include original user request reminder, got: {}",
                format,
                result
            );
            assert!(
                result.contains(user_prompt),
                "Format {:?} should tell model to retry with corrected parameters, got: {}",
                format,
                result
            );
        }
    }

    #[test]
    fn test_format_tool_result_sql_error_includes_specific_guidance() {
        let call = ParsedToolCall {
            server: "builtin".to_string(),
            tool: "sql_select".to_string(),
            arguments: json!({"sql": "SELECT TO_CHAR(date, 'YYYY-MM') FROM sales"}),
            raw: "".to_string(),
            id: None,
        };

        let error_msg = r#"{"success": false, "error": "Function not found: TO_CHAR", "sql_executed": "SELECT TO_CHAR..."}"#;
        let user_prompt = "what are my 2025 sales by month?";

        let result = format_tool_result(&call, error_msg, true, ToolFormat::Hermes, Some(user_prompt));
        
        // Should include SQL-specific error guidance
        assert!(
            result.contains("SQL ERROR"),
            "Should include SQL ERROR, got: {}",
            result
        );
        assert!(
            result.contains("CAST") || result.contains("TO_CHAR"),
            "Should mention BigQuery-specific alternatives, got: {}",
            result
        );
        // Should include the original user prompt
        assert!(
            result.contains(user_prompt),
            "Should include original user prompt, got: {}",
            result
        );
        assert!(
            result.contains("Original User Request"),
            "Should label the original request, got: {}",
            result
        );
    }

    #[test]
    fn test_parse_markdown_json_tool_call() {
        // Test case based on actual model output
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

        let calls = parse_hermes_tool_calls(content);
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
        // Test markdown code block without language specifier
        let content = r#"
```
{"name": "test_tool", "arguments": {"param": "value"}}
```
"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "test_tool");
    }

    #[test]
    fn test_parse_markdown_json_ignores_non_tool_json() {
        // Should not match JSON without "name" field
        let content = r#"Here's some config:

```json
{
  "database": "postgres",
  "port": 5432
}
```
"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(
            calls.len(),
            0,
            "Should not parse non-tool JSON as tool calls"
        );
    }

    #[test]
    fn test_parse_gpt_oss_legacy_format() {
        // GPT-OSS uses tool_name and tool_args instead of name and arguments
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
    fn test_parse_hermes_with_stray_closing_bracket() {
        // Some models (like Phi-4-mini) may output a stray `>` after the JSON content
        // This happens when the model confuses XML self-closing tag syntax
        // Note the extra `>` after `}}` before `</tool_call>`
        let content = r#"<tool_call>{"name": "sql_select", "arguments": {"sql": "SELECT SUM(total_sale) FROM table WHERE period >= '2025-09-01' AND period < '2025-10-01'"}}></tool_call>"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse tool call with stray > character");
        assert_eq!(calls[0].tool, "sql_select");
        assert!(calls[0].arguments.get("sql").is_some(), "Should extract sql argument");
    }

    #[test]
    fn test_parse_hermes_with_self_closing_style() {
        // Handle when model tries to use self-closing tag style like <tool_call ... />
        let content = r#"<tool_call>{"name": "test_tool", "arguments": {"x": 1}}/></tool_call>"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse tool call with /> characters");
        assert_eq!(calls[0].tool, "test_tool");
    }

    #[test]
    fn test_parse_llama_parameters_format() {
        // Llama uses "parameters" instead of "arguments"
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
    fn test_parse_braintrust_function_format() {
        // Braintrust Llama recipe uses <function=name>{...}</function>
        let content = r#"Let me check the weather.
<function=get_weather>{"location": "Tokyo, JP"}</function>
The weather is..."#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_weather");
        assert_eq!(
            calls[0].arguments.get("location").and_then(|v| v.as_str()),
            Some("Tokyo, JP")
        );
    }

    #[test]
    fn test_parse_gemma_function_call_format() {
        // Gemma uses <function_call> tags like Granite
        let content = r#"<function_call>{"name": "get_product_details", "arguments": {"product_id": "1234"}}</function_call>"#;

        let calls = parse_granite_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "get_product_details");
        assert_eq!(
            calls[0]
                .arguments
                .get("product_id")
                .and_then(|v| v.as_str()),
            Some("1234")
        );
    }

    #[test]
    fn test_parse_markdown_gpt_oss_format() {
        // GPT-OSS in markdown code block with legacy field names
        let content = r#"
```json
{"tool_name": "calculate", "tool_args": {"expression": "2 + 2"}}
```
"#;

        let calls = parse_hermes_tool_calls(content);
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

    #[test]
    fn test_parse_pythonic_code_block_sql_select() {
        // Test Pythonic function call in a plaintext code block (Phi-4 style output)
        let content = r#"```plaintext
sql_select("SELECT SUM(total_sale) FROM table WHERE year = 2025", ["bq-123"])
```"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Expected 1 tool call, found {}", calls.len());
        assert_eq!(calls[0].tool, "sql_select");
        assert!(calls[0].arguments.get("sql").is_some() || !calls[0].arguments.is_null());
    }

    #[test]
    fn test_parse_pythonic_code_block_schema_search() {
        // Test schema_search in a code block
        let content = r#"```
schema_search("customer orders")
```"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "schema_search");
    }

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

    // ============ Python Code Detection Tests ============

    #[test]
    fn test_detect_explicit_python_block() {
        let content = r#"Let me calculate that for you.

```python
import math
result = math.sqrt(16)
print(f"Result: {result}")
```

The answer is 4."#;

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 1);
        assert!(detected[0].explicit_python);
        assert!(detected[0].code.contains("import math"));
        assert!(detected[0].code.contains("print"));
    }

    #[test]
    fn test_detect_py_short_form() {
        let content = "```py\nprint('hello')\n```";

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 1);
        assert!(detected[0].explicit_python);
        assert_eq!(detected[0].code, "print('hello')");
    }

    #[test]
    fn test_detect_implicit_python_block() {
        let content = r#"Here's a simple calculation:

```
result = 17 * 23 + 456
print(result)
```

Done."#;

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 1);
        assert!(!detected[0].explicit_python);
        assert!(detected[0].code.contains("print"));
    }

    #[test]
    fn test_detect_import_statement() {
        let content = "```\nimport json\ndata = json.loads('{}')\n```";

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 1);
        assert!(detected[0].code.contains("import json"));
    }

    #[test]
    fn test_detect_multiple_blocks() {
        let content = r#"First:
```python
x = 1
```

Second:
```python
y = 2
```"#;

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 2);
        assert!(detected[0].code.contains("x = 1"));
        assert!(detected[1].code.contains("y = 2"));
    }

    #[test]
    fn test_ignore_non_python_blocks() {
        let content = r#"JavaScript code:

```javascript
const x = 1;
console.log(x);
```

Rust code:

```rust
fn main() {
    println!("Hello");
}
```
"#;

        let detected = detect_python_code(content);
        assert_eq!(detected.len(), 0);
    }

    #[test]
    fn test_looks_like_python_patterns() {
        // Should detect as Python
        assert!(looks_like_python("import math"));
        assert!(looks_like_python("from collections import Counter"));
        assert!(looks_like_python("def foo():\n    pass"));
        assert!(looks_like_python("print('hello')"));
        assert!(looks_like_python("for x in range(10):\n    print(x)"));
        assert!(looks_like_python("[x*2 for x in range(5)]"));
        assert!(looks_like_python("f'Hello {name}'"));

        // Should NOT detect as Python
        assert!(!looks_like_python("const x = 1;"));
        assert!(!looks_like_python("function foo() {}"));
        assert!(!looks_like_python("fn main() {}"));
        assert!(!looks_like_python("SELECT * FROM users"));
        assert!(!looks_like_python("$variable = 'value';"));
    }

    #[test]
    fn test_dedent_code() {
        let code = "    x = 1\n    y = 2\n    print(x + y)";
        let dedented = dedent_code(code);
        assert_eq!(dedented, "x = 1\ny = 2\nprint(x + y)");
    }

    // ============ SQL Select Positional Argument Tests ============

    #[test]
    fn test_sql_select_positional_single_string() {
        // Single positional argument (just the SQL query)
        let args_str = r#""SELECT * FROM orders WHERE year = 2025""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        assert_eq!(sql, Some("SELECT * FROM orders WHERE year = 2025"));
    }

    #[test]
    fn test_sql_select_positional_with_source_array() {
        // Positional: sql_select("SELECT ...", ["source_id"])
        let args_str = r#""SELECT SUM(total_sale) FROM table WHERE year = 2025", ["bq-123"]"#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        let source_id = result.get("source_id").and_then(|v| v.as_str());
        
        assert_eq!(sql, Some("SELECT SUM(total_sale) FROM table WHERE year = 2025"));
        assert_eq!(source_id, Some("bq-123"));
    }

    #[test]
    fn test_sql_select_positional_with_source_string() {
        // Positional: sql_select("SELECT ...", "source_id")
        let args_str = r#""SELECT * FROM users", "postgres-db""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        let source_id = result.get("source_id").and_then(|v| v.as_str());
        
        assert_eq!(sql, Some("SELECT * FROM users"));
        assert_eq!(source_id, Some("postgres-db"));
    }

    #[test]
    fn test_sql_select_positional_with_equals_in_sql() {
        // SQL with multiple = signs that could confuse the parser
        let args_str = r#""SELECT SUM(total_sale) FROM sales WHERE EXTRACT(MONTH FROM period) = 10 AND EXTRACT(YEAR FROM period) = 2025""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        assert_eq!(sql, Some("SELECT SUM(total_sale) FROM sales WHERE EXTRACT(MONTH FROM period) = 10 AND EXTRACT(YEAR FROM period) = 2025"));
    }

    #[test]
    fn test_sql_select_named_arguments_still_work() {
        // Named arguments should still work
        let args_str = r#"sql="SELECT * FROM users", source_id="pg-123""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        // This should fall through to parse_pythonic_arguments
        assert!(result.is_object());
    }

    #[test]
    fn test_extract_positional_arguments() {
        // Test the underlying positional argument extraction
        let args = extract_positional_arguments(r#""hello", "world", 42"#);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0].as_str(), Some("hello"));
        assert_eq!(args[1].as_str(), Some("world"));
        assert_eq!(args[2].as_i64(), Some(42));
    }

    #[test]
    fn test_extract_positional_with_nested_arrays() {
        // Nested arrays shouldn't be split on commas inside them
        let args = extract_positional_arguments(r#""query text", ["a", "b", "c"]"#);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].as_str(), Some("query text"));
        assert!(args[1].is_array());
    }

    #[test]
    fn test_parse_pythonic_code_block_sql_select_positional() {
        // Test the full flow: code block with positional sql_select call
        let content = r#"```plaintext
sql_select("SELECT SUM(total_sale) FROM table WHERE year = 2025", ["bq-123"])
```"#;

        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Expected 1 tool call, found {}", calls.len());
        assert_eq!(calls[0].tool, "sql_select");
        
        let sql = calls[0].arguments.get("sql").and_then(|v| v.as_str());
        let source_id = calls[0].arguments.get("source_id").and_then(|v| v.as_str());
        
        assert!(sql.is_some(), "SQL should be extracted");
        assert!(sql.unwrap().contains("SELECT SUM"), "SQL should contain the query");
        assert_eq!(source_id, Some("bq-123"), "source_id should be extracted");
    }

    // ============ Tolerance Tests ============

    #[test]
    fn test_fix_llm_json_python_booleans() {
        let input = r#"{"name": "test", "arguments": {"flag": True, "other": False}}"#;
        let fixed = fix_llm_json(input);
        assert!(fixed.contains("true"));
        assert!(fixed.contains("false"));
        assert!(!fixed.contains("True"));
        assert!(!fixed.contains("False"));
    }

    #[test]
    fn test_fix_llm_json_python_none() {
        let input = r#"{"name": "test", "arguments": {"value": None}}"#;
        let fixed = fix_llm_json(input);
        assert!(fixed.contains("null"));
        assert!(!fixed.contains("None"));
    }

    #[test]
    fn test_fix_llm_json_comments() {
        // Block comments
        let input = r#"{"name": "test" /* this is a comment */, "arguments": {}}"#;
        let fixed = fix_llm_json(input);
        assert!(!fixed.contains("/*"));
        assert!(!fixed.contains("*/"));
        assert!(!fixed.contains("comment"));
    }

    #[test]
    fn test_parse_flexible_json_unquoted_keys() {
        // json5 should handle unquoted keys
        let input = r#"{name: "test_tool", arguments: {}}"#;
        let parsed = parse_flexible_json(input);
        assert!(parsed.is_some(), "Should parse JSON with unquoted keys");
        let val = parsed.unwrap();
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test_tool"));
    }

    #[test]
    fn test_unwrap_structure_single_element_array() {
        let input = r#"[{"name": "test", "arguments": {}}]"#;
        let parsed = parse_flexible_json(input);
        assert!(parsed.is_some());
        let val = parsed.unwrap();
        // Should be unwrapped to the inner object
        assert!(val.is_object(), "Should unwrap single-element array");
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test"));
    }

    #[test]
    fn test_unwrap_structure_nested_wrapper() {
        let input = r#"{"tool_call": {"name": "test", "arguments": {}}}"#;
        let parsed = parse_flexible_json(input);
        assert!(parsed.is_some());
        let val = parsed.unwrap();
        // Should be unwrapped to the inner object
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test"));
    }

    #[test]
    fn test_parse_hermes_case_insensitive() {
        // Test uppercase
        let content = r#"<TOOL_CALL>{"name": "test", "arguments": {}}</TOOL_CALL>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse uppercase TOOL_CALL");
        assert_eq!(calls[0].tool, "test");
    }

    #[test]
    fn test_parse_hermes_mixed_case() {
        let content = r#"<Tool_Call>{"name": "test", "arguments": {}}</Tool_Call>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse mixed case Tool_Call");
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
    fn test_parse_hermes_typo_tool_dash_call() {
        let content = r#"<tool-call>{"name": "test", "arguments": {}}</tool-call>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse typo 'tool-call'");
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
    fn test_find_json_objects_in_text() {
        let content = r#"Here's the tool call: {"name": "test", "arguments": {"x": 1}} and some more text"#;
        let objects = find_json_objects(content);
        assert_eq!(objects.len(), 1, "Should find one JSON object");
        assert!(objects[0].contains("\"name\""));
    }

    #[test]
    fn test_extract_tool_call_by_regex() {
        let content = r#"broken json here "name": "my_tool", "arguments": {"x": 1} more garbage"#;
        let call = extract_tool_call_by_regex(content);
        assert!(call.is_some(), "Should extract via regex fallback");
        let c = call.unwrap();
        assert_eq!(c.tool, "my_tool");
    }

    #[test]
    fn test_extract_tool_name_nested_paths() {
        // Test nested tool.name path
        let input = json!({"tool": {"name": "nested_tool"}});
        let name = extract_tool_name(&input);
        assert_eq!(name, Some("nested_tool".to_string()));
    }

    #[test]
    fn test_extract_tool_name_function_alias() {
        let input = json!({"function": "my_function", "arguments": {}});
        let name = extract_tool_name(&input);
        assert_eq!(name, Some("my_function".to_string()));
    }

    #[test]
    fn test_extract_tool_name_tool_as_string() {
        // When "tool" is a string value (not an object), use it as the name
        let input = json!({"tool": "direct_tool_name", "args": {}});
        let name = extract_tool_name(&input);
        assert_eq!(name, Some("direct_tool_name".to_string()));
    }

    #[test]
    fn test_json_with_leading_garbage() {
        // find_json_objects should extract JSON from anywhere in content
        let content = r#"Here is some explanation text and then {"name": "test", "arguments": {}} more text"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should find JSON in text via fallback");
        assert_eq!(calls[0].tool, "test");
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
    fn test_parse_with_python_none_in_arguments() {
        let content = r#"<tool_call>{"name": "test", "arguments": {"value": None}}</tool_call>"#;
        let calls = parse_hermes_tool_calls(content);
        assert_eq!(calls.len(), 1, "Should parse with Python None");
        assert!(calls[0].arguments.get("value").map(|v| v.is_null()).unwrap_or(false));
    }
}
