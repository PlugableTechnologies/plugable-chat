//! JSON repair utilities for handling malformed LLM output.
//!
//! LLMs often produce JSON with common issues:
//! - Python booleans (True/False instead of true/false)
//! - Python None instead of null
//! - Trailing commas
//! - Comments
//! - Unescaped newlines in strings

use regex::Regex;
use serde_json::Value;

/// Repair common JSON issues from LLMs.
/// Handles: trailing commas, Python booleans/None, BOM, comments, unescaped newlines.
pub fn repair_malformed_json(json_str: &str) -> String {
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
/// 2. repair_malformed_json preprocessing + serde_json
/// 3. Single quote replacement + serde_json
/// 4. json5 parser (handles unquoted keys, comments, trailing commas)
/// 5. Balanced brace extraction + retry
pub fn parse_json_lenient(raw: &str) -> Option<Value> {
    // Fast path: try direct parse
    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        return Some(unwrap_json_structure(val));
    }

    // Fix trivial JSON issues first
    let fixed = repair_malformed_json(raw);
    if let Ok(val) = serde_json::from_str::<Value>(&fixed) {
        return Some(unwrap_json_structure(val));
    }

    // Fallback: try replacing single quotes with double quotes
    let single_to_double = fixed.replace('\'', "\"");
    if let Ok(val) = serde_json::from_str::<Value>(&single_to_double) {
        return Some(unwrap_json_structure(val));
    }

    // Fallback: try json5 parser (handles unquoted keys, comments, etc.)
    if let Ok(val) = json5::from_str::<Value>(&fixed) {
        return Some(unwrap_json_structure(val));
    }

    // Fallback: try extracting balanced braces and retry
    if let Some(balanced) = extract_balanced_json_braces(raw.trim()) {
        if balanced != raw {
            let fixed_balanced = repair_malformed_json(&balanced);
            if let Ok(val) = serde_json::from_str::<Value>(&fixed_balanced) {
                return Some(unwrap_json_structure(val));
            }
            if let Ok(val) = json5::from_str::<Value>(&fixed_balanced) {
                return Some(unwrap_json_structure(val));
            }
        }
    }

    None
}

/// Unwrap common structural wrappers from parsed JSON.
/// Handles:
/// - Single-element arrays: [{"name": ...}] -> {"name": ...}
/// - Nested wrappers: {"tool_call": {"name": ...}} -> {"name": ...}
pub fn unwrap_json_structure(value: Value) -> Value {
    // Unwrap single-element arrays
    if let Value::Array(arr) = &value {
        if arr.len() == 1 {
            return unwrap_json_structure(arr[0].clone());
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
                    return unwrap_json_structure(inner.clone());
                }
            }
        }
    }

    value
}

/// Extract a balanced {} block from the start of a string
pub fn extract_balanced_json_braces(s: &str) -> Option<String> {
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
pub fn find_json_objects_in_content(content: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '{' {
            // Try to extract balanced braces starting from this position
            if let Some(json_str) = extract_balanced_json_braces(&content[i..]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repair_malformed_json_python_booleans() {
        let input = r#"{"name": "test", "arguments": {"flag": True, "other": False}}"#;
        let fixed = repair_malformed_json(input);
        assert!(fixed.contains("true"));
        assert!(fixed.contains("false"));
        assert!(!fixed.contains("True"));
        assert!(!fixed.contains("False"));
    }

    #[test]
    fn test_repair_malformed_json_python_none() {
        let input = r#"{"name": "test", "arguments": {"value": None}}"#;
        let fixed = repair_malformed_json(input);
        assert!(fixed.contains("null"));
        assert!(!fixed.contains("None"));
    }

    #[test]
    fn test_repair_malformed_json_comments() {
        let input = r#"{"name": "test" /* this is a comment */, "arguments": {}}"#;
        let fixed = repair_malformed_json(input);
        assert!(!fixed.contains("/*"));
        assert!(!fixed.contains("*/"));
        assert!(!fixed.contains("comment"));
    }

    #[test]
    fn test_parse_json_lenient_unquoted_keys() {
        let input = r#"{name: "test_tool", arguments: {}}"#;
        let parsed = parse_json_lenient(input);
        assert!(parsed.is_some(), "Should parse JSON with unquoted keys");
        let val = parsed.unwrap();
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test_tool"));
    }

    #[test]
    fn test_unwrap_json_structure_single_element_array() {
        let input = r#"[{"name": "test", "arguments": {}}]"#;
        let parsed = parse_json_lenient(input);
        assert!(parsed.is_some());
        let val = parsed.unwrap();
        assert!(val.is_object(), "Should unwrap single-element array");
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test"));
    }

    #[test]
    fn test_unwrap_json_structure_nested_wrapper() {
        let input = r#"{"tool_call": {"name": "test", "arguments": {}}}"#;
        let parsed = parse_json_lenient(input);
        assert!(parsed.is_some());
        let val = parsed.unwrap();
        assert_eq!(val.get("name").and_then(|v| v.as_str()), Some("test"));
    }

    #[test]
    fn test_find_json_objects_in_content() {
        let content = r#"Here's the tool call: {"name": "test", "arguments": {"x": 1}} and some more text"#;
        let objects = find_json_objects_in_content(content);
        assert_eq!(objects.len(), 1, "Should find one JSON object");
        assert!(objects[0].contains("\"name\""));
    }
}
