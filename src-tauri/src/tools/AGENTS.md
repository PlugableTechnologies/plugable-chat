# Built-in Tools & Python Execution

## Python Execution Philosophy

Python programs in Plugable Chat serve two purposes:

1. **Direct Calculation & Display**: Execute computations and immediately display results to the user via stdout, followed by the model providing helpful commentary on those results.

2. **Tool Orchestration**: Call allowed Python intrinsics/imports plus any harness-enabled tool functions (discovered via `tool_search` or directly available).

### Output Routing

| Stream | Destination | Behavior |
|--------|-------------|----------|
| **stdout** | User chat | Results displayed directly to user in the conversation |
| **stderr** | Tool accordion | Kept in the collapsible tool result; sent to model for another turn with a prompt to resolve the error |

### Execution Modes

Python code can be executed in two modes, determined by the tool call format:

| Mode | Format | How Model Invokes | Prompt Instructs |
|------|--------|-------------------|------------------|
| **Native Tool Mode** | `ToolCallFormatName::Native` | Calls `python_execution(code=[...])` as a tool | Model to call `python_execution` tool with code array |
| **Text/Code Mode** | Other formats (Hermes, CodeMode, etc.) | Outputs raw ` ```python ` blocks | Model to output raw Python in fenced blocks |

**Native Mode**: The model calls `python_execution` as a tool with the `code` parameter (array of lines). This integrates with the standard tool calling flow.

**Text Mode**: The model outputs raw ` ```python ` code blocks. The agentic loop detects these blocks and automatically wraps them in a `python_execution` call.

### Detection & Execution

Python code blocks are automatically detected and executed when:
- `python_execution_enabled` is true in settings
- `python_tool_calling_enabled` is true
- The tool filter allows `python_execution`

The system prompt adapts to the tool call format, instructing the model to either call the tool directly (native) or output raw code blocks (text mode).

## Code Mode Philosophy (Advanced Tool Use)
- Code Mode implementation: discover tools on-demand with `tool_search`, orchestrate them in Python (`python_execution`).
- Goals: lean prompts, deferred tool loading, multi-step work in Python.
- Guidance: remind models to search first, then return one runnable Python program calling discovered tools.

## Python Sandbox Configuration Sync
The Python code execution sandbox has allowed/disallowed modules and builtins defined in multiple locations that must be kept in sync:

1. **Input Validation** (`tools/code_execution.rs`)
   - `ALLOWED_MODULES` constant - duplicated list for pre-execution validation.
   - `validate_input()` - blocks dangerous patterns like `__import__`, `eval(`, `exec(`, `compile(`.
   - `check_imports()` - validates imports before code reaches the sandbox.

2. **Model Prompts** (`lib.rs`)
   - `build_system_prompt()` accurately describes allowed modules and restrictions.

## Current Allowed Modules
```
math, json, random, re, datetime, collections, itertools, functools,
operator, string, textwrap, copy, types, typing, abc, numbers,
decimal, fractions, statistics, hashlib, base64, binascii, html
```

## Guardrail: Sync Process
When modifying Python features:
1. Update `sandbox.rs` first (source of truth).
2. Update `code_execution.rs` (keep `ALLOWED_MODULES` identical).
3. Update system prompts in `lib.rs`.
4. Run tests: `cargo test code_execution`.
