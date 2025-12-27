# Built-in Tools & Python Execution

## Code Mode Philosophy (Advanced Tool Use)
- Code Mode implementation: discover tools on-demand with `tool_search`, orchestrate them in Python (`code_execution`).
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
