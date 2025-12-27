//! Python code processing utilities for the agentic loop.
//!
//! This module provides utilities for parsing, fixing, and validating Python
//! code produced by LLMs. It handles common issues like missing indentation,
//! unsupported syntax, and various argument format variations.

use crate::tools::code_execution::CodeExecutionInput;
use regex::Regex;
use rustpython_parser::{ast, Parse};
use serde_json;

/// Parse python_execution arguments, handling multiple formats from different models.
///
/// Models may produce different argument structures:
/// - Correct: `{"code": ["line1", "line2"], "context": null}`
/// - Direct array: `["line1", "line2"]` (model put code directly in arguments)
/// - Nested: `{"arguments": {"code": [...]}}` (double-wrapped)
pub fn parse_python_execution_args(arguments: &serde_json::Value) -> CodeExecutionInput {
    // First, try standard format: {"code": [...], "context": ...}
    if let Ok(mut input) = serde_json::from_value::<CodeExecutionInput>(arguments.clone()) {
        if !input.code.is_empty() {
            println!(
                "[python_execution] Parsed standard format: {} lines",
                input.code.len()
            );
            input.code = fix_python_indentation(&input.code);
            return input;
        }
    }

    // Try direct array format: arguments is already the code array
    if let Some(arr) = arguments.as_array() {
        let code: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        if !code.is_empty() {
            println!(
                "[python_execution] Parsed direct array format: {} lines",
                code.len()
            );
            let fixed_code = fix_python_indentation(&code);
            return CodeExecutionInput {
                code: fixed_code,
                context: None,
            };
        }
    }

    // Try double-wrapped: {"arguments": {"code": [...]}} or {"code": {"code": [...]}}
    if let Some(inner) = arguments.get("arguments").or_else(|| arguments.get("code")) {
        if let Some(arr) = inner.as_array() {
            let code: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            if !code.is_empty() {
                println!(
                    "[python_execution] Parsed double-wrapped format: {} lines",
                    code.len()
                );
                let fixed_code = fix_python_indentation(&code);
                return CodeExecutionInput {
                    code: fixed_code,
                    context: None,
                };
            }
        } else if let Ok(mut input) = serde_json::from_value::<CodeExecutionInput>(inner.clone()) {
            if !input.code.is_empty() {
                println!(
                    "[python_execution] Parsed nested format: {} lines",
                    input.code.len()
                );
                input.code = fix_python_indentation(&input.code);
                return input;
            }
        }
    }

    // Log the actual format received for debugging
    let preview: String = serde_json::to_string(arguments)
        .unwrap_or_else(|_| "???".to_string())
        .chars()
        .take(300)
        .collect();
    println!(
        "[python_execution] Could not parse arguments, got: {}",
        preview
    );

    // Return empty input - this will be caught by validation
    CodeExecutionInput {
        code: vec![],
        context: None,
    }
}

/// Fix missing Python indentation in code lines.
///
/// When models output code as arrays of lines, they often omit indentation.
/// This function uses a simple heuristic: track indent level based on
/// block-starting keywords (for, if, while, def, etc.) and keywords that
/// indicate staying at the same or reduced level (else, elif, return, etc.).
///
/// This is a best-effort fix and may not handle all edge cases perfectly.
pub fn fix_python_indentation(lines: &[String]) -> Vec<String> {
    // Patterns that start a block (require indented lines after)
    let block_starters = Regex::new(
        r"^\s*(for\s+.+:|while\s+.+:|if\s+.+:|elif\s+.+:|else\s*:|def\s+.+:|class\s+.+:|try\s*:|except.*:|finally\s*:|with\s+.+:)\s*(#.*)?$"
    ).unwrap();

    // Patterns that should be at same level as opening (else, elif, except, finally)
    let dedent_before =
        Regex::new(r"^\s*(elif\s+.+:|else\s*:|except.*:|finally\s*:)\s*(#.*)?$").unwrap();

    // Statements that typically end a block
    let block_enders = Regex::new(r"^\s*(return\b|break\b|continue\b|raise\b|pass\b)").unwrap();

    let mut result = Vec::with_capacity(lines.len());
    let mut indent_stack: Vec<usize> = vec![0]; // Stack of indent levels
    let indent_str = "    "; // 4 spaces

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        // Check if line already has indentation
        let existing_indent = line.len() - line.trim_start().len();
        if existing_indent > 0 {
            // Line already has indentation - trust it and reset our tracking
            result.push(line.clone());
            let indent_units = existing_indent / 4;
            indent_stack.clear();
            indent_stack.push(indent_units);
            if block_starters.is_match(trimmed) {
                indent_stack.push(indent_units + 1);
            }
            continue;
        }

        // Get current indent level
        let current_indent = *indent_stack.last().unwrap_or(&0);

        // Check if this line should be at reduced indent (else, elif, except, finally)
        let line_indent = if dedent_before.is_match(trimmed) {
            // Pop one level for else/elif/except/finally
            if indent_stack.len() > 1 {
                indent_stack.pop();
            }
            *indent_stack.last().unwrap_or(&0)
        } else {
            current_indent
        };

        // Apply indentation
        let indented_line = if line_indent > 0 {
            format!("{}{}", indent_str.repeat(line_indent), trimmed)
        } else {
            trimmed.to_string()
        };

        result.push(indented_line);

        // Check if next line needs more indent (this line starts a block)
        if block_starters.is_match(trimmed) {
            indent_stack.push(line_indent + 1);
        } else if block_enders.is_match(trimmed) {
            // After return/break/continue/pass/raise, next line might be less indented
            // But only pop if we're not at top level and there's a next line
            if indent_stack.len() > 1 && i + 1 < lines.len() {
                // Peek at next line - if it's a block continuation keyword, don't pop
                let next_trimmed = lines[i + 1].trim();
                if !dedent_before.is_match(next_trimmed) {
                    indent_stack.pop();
                }
            }
        }
    }

    // Check if any indentation was applied
    let had_changes = result.iter().zip(lines.iter()).any(|(a, b)| a != b);
    if had_changes {
        println!("[python_execution] Auto-fixed Python indentation");
    }

    result
}

/// Strip unsupported Python keywords/patterns that cause RustPython compilation errors.
///
/// Keywords removed:
/// - `await` - RustPython sandbox doesn't run in async context
///
/// This is called before code execution to handle models that add unsupported syntax.
pub fn strip_unsupported_python(lines: &[String]) -> Vec<String> {
    // Pattern to match standalone `await` keyword (not inside strings)
    // Matches: `await foo()`, `x = await bar()`, but not `"await"` or `# await`
    let await_pattern = Regex::new(r"\bawait\s+").unwrap();

    let mut result = Vec::with_capacity(lines.len());
    let mut stripped_count = 0;

    for line in lines {
        let trimmed = line.trim();

        // Skip comments and string-only lines
        if trimmed.starts_with('#') {
            result.push(line.clone());
            continue;
        }

        // Strip `await ` from the line
        if await_pattern.is_match(line) {
            let fixed = await_pattern.replace_all(line, "").to_string();
            result.push(fixed);
            stripped_count += 1;
        } else {
            result.push(line.clone());
        }
    }

    if stripped_count > 0 {
        println!(
            "[python_execution] Stripped {} `await` keyword(s) (not needed in sandbox)",
            stripped_count
        );
    }

    result
}

/// Extract a Python program from a model response.
///
/// Looks for Python code blocks (```python ... ```) or standalone code patterns.
/// Returns the extracted code as a vector of lines, or None if no valid code found.
pub fn extract_python_program(response: &str) -> Option<Vec<String>> {
    // Pattern 1: Code block with python marker
    let code_block_pattern = Regex::new(r"```python\s*\n([\s\S]*?)```").ok()?;

    if let Some(caps) = code_block_pattern.captures(response) {
        let code = caps.get(1)?.as_str();
        let lines: Vec<String> = code.lines().map(|s| s.to_string()).collect();
        if !lines.is_empty() {
            return Some(lines);
        }
    }

    // Pattern 2: Code block without language marker (but looks like Python)
    let generic_block_pattern = Regex::new(r"```\s*\n([\s\S]*?)```").ok()?;

    if let Some(caps) = generic_block_pattern.captures(response) {
        let code = caps.get(1)?.as_str();
        let lines: Vec<String> = code.lines().map(|s| s.to_string()).collect();

        // Validate it looks like Python
        if !lines.is_empty() && looks_like_python(&lines) {
            return Some(lines);
        }
    }

    // Pattern 3: Detect inline Python code (without code blocks)
    // Only if it looks like a complete program
    let trimmed = response.trim();
    if looks_like_standalone_python(trimmed) {
        let lines: Vec<String> = trimmed.lines().map(|s| s.to_string()).collect();
        if !lines.is_empty() {
            return Some(lines);
        }
    }

    None
}

/// Quick heuristic to check if code looks like Python
fn looks_like_python(lines: &[String]) -> bool {
    let python_indicators = [
        "def ", "import ", "from ", "class ", "print(", "if __name__", "for ", "while ", "try:",
        "except", "elif ", "else:", "return ", "with ",
    ];

    lines.iter().any(|line| {
        let trimmed = line.trim();
        python_indicators
            .iter()
            .any(|ind| trimmed.starts_with(ind) || trimmed.contains(ind))
    })
}

/// Check if text looks like standalone Python code (not prose)
fn looks_like_standalone_python(text: &str) -> bool {
    // Must have at least some code structure
    if text.lines().count() < 2 {
        return false;
    }

    // Count lines that look like code vs prose
    let mut code_lines = 0;
    let mut prose_lines = 0;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Code indicators
        if trimmed.starts_with("def ")
            || trimmed.starts_with("import ")
            || trimmed.starts_with("from ")
            || trimmed.starts_with("class ")
            || trimmed.starts_with("for ")
            || trimmed.starts_with("if ")
            || trimmed.starts_with("while ")
            || trimmed.starts_with("return ")
            || trimmed.starts_with("print(")
            || trimmed.ends_with(":")
            || trimmed.contains(" = ")
            || trimmed.contains("()")
        {
            code_lines += 1;
        } else if trimmed.len() > 50 && trimmed.contains(' ') && !trimmed.contains('(') {
            // Long lines without function calls are likely prose
            prose_lines += 1;
        }
    }

    // Require code to significantly outweigh prose
    code_lines > 0 && code_lines > prose_lines * 2
}

/// Quick syntax validation for Python code before execution to avoid looping on non-code text.
pub fn is_valid_python_syntax(code_lines: &[String]) -> bool {
    let code = code_lines.join("\n");
    match ast::Suite::parse(&code, "<embedded>") {
        Ok(_) => true,
        Err(err) => {
            println!(
                "[PythonSyntaxCheck] Skipping python_execution due to parse error: {}",
                err
            );
            false
        }
    }
}

/// Reconstruct SQL from malformed sql_select arguments.
///
/// When models call sql_select incorrectly (e.g., positional arguments parsed
/// as key-value pairs due to '=' in SQL), the arguments may look like:
/// `{"\"SELECT ... WHERE x": "10 AND y = 20\""}`
///
/// This function attempts to reconstruct the original SQL by:
/// 1. Detecting if keys look like SQL fragments (contain SELECT, WHERE, etc.)
/// 2. Joining keys and values with '=' to reconstruct the query
///
/// Returns None if the arguments don't look like malformed SQL.
pub fn reconstruct_sql_from_malformed_args(arguments: &serde_json::Value) -> Option<String> {
    let obj = arguments.as_object()?;

    // Skip if it already has the proper sql key with a non-empty value
    if let Some(sql_val) = obj.get("sql") {
        if let Some(s) = sql_val.as_str() {
            if !s.is_empty() {
                return None;
            }
        }
    }

    // Look for keys that look like SQL fragments
    let sql_keywords = [
        "SELECT", "INSERT", "UPDATE", "DELETE", "FROM", "WHERE", "JOIN",
    ];

    let mut sql_fragments: Vec<(String, String)> = Vec::new();

    for (key, value) in obj.iter() {
        // Skip known proper parameter names
        if key == "sql" || key == "source_id" || key == "parameters" || key == "max_rows" {
            continue;
        }

        let key_upper = key.to_uppercase();

        // Check if the key looks like it contains SQL
        let looks_like_sql = sql_keywords.iter().any(|kw| key_upper.contains(kw))
            || key.contains('(') // Function calls like EXTRACT(...)
            || key.contains('"') // Quoted strings
            || key.starts_with("\""); // Malformed quoted key

        if looks_like_sql {
            let val_str = match value {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => serde_json::to_string(value).unwrap_or_default(),
            };
            sql_fragments.push((key.clone(), val_str));
        }
    }

    if sql_fragments.is_empty() {
        return None;
    }

    // Reconstruct the SQL by joining fragments
    // The malformed parsing typically splits on '=' so we join with '='
    let mut reconstructed = String::new();
    for (i, (key, value)) in sql_fragments.iter().enumerate() {
        // Clean up the key (remove surrounding quotes if present)
        let clean_key = key
            .trim_start_matches('"')
            .trim_end_matches('"')
            .to_string();

        // Clean up the value (remove surrounding quotes if present)
        let clean_value = value
            .trim_start_matches('"')
            .trim_end_matches('"')
            .to_string();

        if i > 0 {
            reconstructed.push(' ');
        }

        reconstructed.push_str(&clean_key);

        // Only add '=' if the value is non-empty and doesn't start with common SQL joiners
        if !clean_value.is_empty() {
            let value_upper = clean_value.trim().to_uppercase();
            let needs_equals = !value_upper.starts_with("AND ")
                && !value_upper.starts_with("OR ")
                && !value_upper.starts_with("FROM ")
                && !value_upper.starts_with("WHERE ")
                && !value_upper.starts_with("GROUP ")
                && !value_upper.starts_with("ORDER ")
                && !value_upper.starts_with("LIMIT ");

            if needs_equals {
                reconstructed.push_str(" = ");
            } else {
                reconstructed.push(' ');
            }
            reconstructed.push_str(&clean_value);
        }
    }

    // Basic validation: must start with SELECT/INSERT/UPDATE/DELETE
    let trimmed_upper = reconstructed.trim().to_uppercase();
    if !trimmed_upper.starts_with("SELECT")
        && !trimmed_upper.starts_with("INSERT")
        && !trimmed_upper.starts_with("UPDATE")
        && !trimmed_upper.starts_with("DELETE")
    {
        println!(
            "[reconstruct_sql_from_malformed_args] Reconstructed text doesn't look like SQL: {}...",
            reconstructed.chars().take(50).collect::<String>()
        );
        return None;
    }

    println!("[reconstruct_sql_from_malformed_args] Successfully reconstructed SQL query");
    Some(reconstructed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fix_python_indentation_if_else() {
        let input = vec![
            "if x > 0:".to_string(),
            "print('positive')".to_string(),
            "else:".to_string(),
            "print('not positive')".to_string(),
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "if x > 0:");
        assert_eq!(result[1], "    print('positive')");
        assert_eq!(result[2], "else:");
        assert_eq!(result[3], "    print('not positive')");
    }

    #[test]
    fn test_fix_python_indentation_nested() {
        let input = vec![
            "for i in range(10):".to_string(),
            "if i % 2 == 0:".to_string(),
            "print('even')".to_string(),
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "for i in range(10):");
        assert_eq!(result[1], "    if i % 2 == 0:");
        assert_eq!(result[2], "        print('even')");
    }

    #[test]
    fn test_fix_python_indentation_preserves_existing() {
        let input = vec![
            "for i in range(10):".to_string(),
            "    print(i)".to_string(), // Already indented - resets tracking
            "print('done')".to_string(), // After explicit indent, we follow it
        ];

        let result = fix_python_indentation(&input);

        assert_eq!(result[0], "for i in range(10):");
        assert_eq!(result[1], "    print(i)"); // Preserved
        // After seeing explicit indent, we reset to that level
        assert_eq!(result[2], "    print('done')");
    }

    #[test]
    fn test_strip_unsupported_await() {
        let input = vec![
            "result = await foo()".to_string(),
            "print(result)".to_string(),
        ];

        let result = strip_unsupported_python(&input);

        assert_eq!(result[0], "result = foo()");
        assert_eq!(result[1], "print(result)");
    }

    #[test]
    fn test_strip_unsupported_preserves_comments() {
        let input = vec![
            "# This is a comment about await".to_string(),
            "result = await foo()".to_string(),
        ];

        let result = strip_unsupported_python(&input);

        assert_eq!(result[0], "# This is a comment about await");
        assert_eq!(result[1], "result = foo()");
    }
}
