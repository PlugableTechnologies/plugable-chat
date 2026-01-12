# Tool Parsing Module

This module handles model-specific tool calling format parsing and result formatting.

## Module Structure

```
tool_parsing/
├── mod.rs              # Re-exports, format_tools_for_model(), parse_tool_calls_for_model_profile()
├── common.rs           # Shared utilities: parse_combined_tool_name(), extract_tool_name_from_json()
├── json_fixer.rs       # JSON repair: repair_malformed_json(), parse_json_lenient()
│
├── hermes_parser.rs    # <tool_call>JSON</tool_call> (Phi, Qwen)
├── harmony_parser.rs   # <|channel|>commentary to=... (GPT-OSS)
├── gemini_parser.rs    # function_call format (Gemini)
├── granite_parser.rs   # <function_call>XML</function_call> (Granite, Gemma)
├── tagged_parser.rs    # [TOOL_CALLS] style (Mistral)
├── pythonic_parser.rs  # tool_name(arg="value") style
├── json_parser.rs      # Pure JSON tool calls
├── markdown_json_parser.rs  # ```json code blocks
├── braintrust_parser.rs     # <function=name>{...}</function> (Llama)
│
├── python_detector.rs  # detect_python_code() for Code Mode
└── result_formatter.rs # format_tool_result() for all formats
```

## Key Functions

### Primary Exports (used by lib.rs)
- `parse_tool_calls_for_model_profile()` - Main entry point for parsing model output
- `format_tool_result()` - Format tool results for injection into chat history
- `detect_python_code()` - Detect Python code blocks for Code Mode
- `parse_combined_tool_name()` - Split "server___tool" into (server, tool)

### Format-Specific Parsers
Each parser handles a specific model family's output format:
- `parse_hermes_tool_calls()` - Most flexible, with many fallbacks
- `parse_harmony_tool_calls()` - GPT-OSS special tokens
- `parse_gemini_tool_calls()` - Gemini function_call
- `parse_granite_tool_calls()` - Granite/Gemma XML
- `parse_tagged_tool_calls()` - [TOOL_CALLS] marker
- `parse_pythonic_tool_calls()` - Python function syntax
- `parse_pure_json_tool_calls()` - Bare JSON objects
- `parse_markdown_json_tool_calls()` - ```json blocks
- `parse_braintrust_function_calls()` - `<function=name>` format

## Fallback Chain

The `parse_hermes_tool_calls()` function implements a comprehensive fallback chain:

1. **Primary**: `<tool_call>...</tool_call>` tags (case-insensitive, with typo tolerance)
2. **Unclosed**: Handle streaming with incomplete closing tags
3. **Tagged**: `[TOOL_CALLS]` Mistral-style markers
4. **Braintrust**: `<function=name>{...}</function>` Llama format
5. **Markdown**: ````json` code blocks
6. **Pythonic Code Block**: `tool_name(args)` in code blocks
7. **Pythonic Bare**: `tool_name(args)` without code blocks
8. **JSON Objects**: Any JSON in content with name-like fields
9. **Regex Fallback**: Extract name/arguments via regex when JSON fails

## JSON Repair (`json_fixer.rs`)

LLMs often produce malformed JSON. The `repair_malformed_json()` function fixes:
- Python booleans (`True`/`False` → `true`/`false`)
- Python None (`None` → `null`)
- Trailing commas
- Line and block comments
- Unescaped newlines in strings
- BOM characters

The `parse_json_lenient()` function tries multiple strategies:
1. Direct serde_json parse
2. After `repair_malformed_json()`
3. Single quotes → double quotes
4. json5 parser (unquoted keys, trailing commas)
5. Balanced brace extraction + retry

## Naming Conventions

Functions follow grep-friendly naming:
- `parse_*_tool_calls()` - Parse a specific format
- `extract_*_from_json()` - Extract a field from JSON
- `repair_*_json()` - Fix malformed JSON
- `format_tool_result()` - Format for output

Variables:
- `tool_call_content` not `content`
- `json_value` not `value`
- `tool_arguments` not `args`

## Testing

Each parser has unit tests in its own file. Run with:
```bash
cargo test -p plugable-chat tool_parsing
```

Key test categories:
- Format-specific parsing (e.g., `test_parse_hermes_tool_call`)
- Tolerance/fallback (e.g., `test_parse_hermes_case_insensitive`)
- JSON repair (e.g., `test_repair_malformed_json_python_booleans`)
- Result formatting (e.g., `test_format_tool_result_hermes`)
