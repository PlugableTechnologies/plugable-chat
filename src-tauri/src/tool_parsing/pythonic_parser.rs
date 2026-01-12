//! Pythonic tool call parser.
//!
//! Parses Pythonic function-call style tool invocations:
//! - `tool_name(arg="value")`
//! - Code blocks with `sql_select("SELECT ...", ["source_id"])`

use regex::Regex;
use serde_json::Value;

use crate::protocol::ParsedToolCall;
use super::common::parse_combined_tool_name;

/// Known tool names to filter out false positives.
/// This prevents Python builtins like print() from being treated as tool calls.
const KNOWN_BUILTIN_TOOLS: [&str; 4] = [
    "sql_select",
    "schema_search",
    "tool_search",
    "python_execution",
];

/// Parse Pythonic function calls inside markdown code blocks.
/// Handles formats like:
/// ```plaintext
/// sql_select("SELECT ...", ["source_id"])
/// ```
/// or similar code blocks with any language tag (python, text, etc.)
pub fn parse_pythonic_code_block_tool_calls(content: &str) -> Vec<ParsedToolCall> {
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

                    // Only parse if it looks like a known tool or has server prefix
                    let is_known = KNOWN_BUILTIN_TOOLS.contains(&name) || name.contains("___");

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

/// Parse Pythonic function-call style tool invocations: `tool_name(arg="value")`
pub fn parse_pythonic_tool_calls(content: &str) -> Vec<ParsedToolCall> {
    let mut calls = Vec::new();
    let re = Regex::new(r"(?m)^\s*([A-Za-z_][A-Za-z0-9_]*)\s*\(([^)]*)\)").unwrap();

    for cap in re.captures_iter(content) {
        let name = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        if name.is_empty() {
            continue;
        }

        // Only parse if it looks like a known tool or has server prefix (e.g., "server___tool")
        let is_known = KNOWN_BUILTIN_TOOLS.contains(&name) || name.contains("___");
        if !is_known {
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

/// Parse positional arguments for sql_select tool.
/// Handles formats like:
/// - sql_select("SELECT ...", ["source_id"]) - positional with array
/// - sql_select("SELECT ...") - just the SQL query
/// - sql_select(sql="SELECT ...", source_id="x") - named arguments
pub fn parse_sql_select_positional_arguments(args_str: &str) -> Value {
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

/// Parse Pythonic named arguments: `arg1="value1", arg2="value2"`
pub fn parse_pythonic_arguments(arg_str: &str) -> Value {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pythonic_code_block_sql_select() {
        let content = r#"```plaintext
sql_select("SELECT SUM(total_sale) FROM table WHERE year = 2025", ["bq-123"])
```"#;

        let calls = parse_pythonic_code_block_tool_calls(content);
        assert_eq!(calls.len(), 1, "Expected 1 tool call, found {}", calls.len());
        assert_eq!(calls[0].tool, "sql_select");
    }

    #[test]
    fn test_parse_pythonic_code_block_schema_search() {
        let content = r#"```
schema_search("customer orders")
```"#;

        let calls = parse_pythonic_code_block_tool_calls(content);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].tool, "schema_search");
    }

    #[test]
    fn test_sql_select_positional_single_string() {
        let args_str = r#""SELECT * FROM orders WHERE year = 2025""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        assert_eq!(sql, Some("SELECT * FROM orders WHERE year = 2025"));
    }

    #[test]
    fn test_sql_select_positional_with_source_array() {
        let args_str = r#""SELECT SUM(total_sale) FROM table WHERE year = 2025", ["bq-123"]"#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        let source_id = result.get("source_id").and_then(|v| v.as_str());
        
        assert_eq!(sql, Some("SELECT SUM(total_sale) FROM table WHERE year = 2025"));
        assert_eq!(source_id, Some("bq-123"));
    }

    #[test]
    fn test_sql_select_positional_with_equals_in_sql() {
        let args_str = r#""SELECT SUM(total_sale) FROM sales WHERE EXTRACT(MONTH FROM period) = 10 AND EXTRACT(YEAR FROM period) = 2025""#;
        let result = parse_sql_select_positional_arguments(args_str);
        
        assert!(result.is_object());
        let sql = result.get("sql").and_then(|v| v.as_str());
        assert_eq!(sql, Some("SELECT SUM(total_sale) FROM sales WHERE EXTRACT(MONTH FROM period) = 10 AND EXTRACT(YEAR FROM period) = 2025"));
    }

    #[test]
    fn test_extract_positional_arguments() {
        let args = extract_positional_arguments(r#""hello", "world", 42"#);
        assert_eq!(args.len(), 3);
        assert_eq!(args[0].as_str(), Some("hello"));
        assert_eq!(args[1].as_str(), Some("world"));
        assert_eq!(args[2].as_i64(), Some(42));
    }

    #[test]
    fn test_extract_positional_with_nested_arrays() {
        let args = extract_positional_arguments(r#""query text", ["a", "b", "c"]"#);
        assert_eq!(args.len(), 2);
        assert_eq!(args[0].as_str(), Some("query text"));
        assert!(args[1].is_array());
    }
}
