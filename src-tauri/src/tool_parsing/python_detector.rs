//! Python code detection in model responses.
//!
//! Detects Python code blocks for the Code Mode tool calling format.

use regex::Regex;

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
pub fn looks_like_python(code: &str) -> bool {
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
}
