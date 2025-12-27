# Python Sandbox Internals

## Source of Truth: `sandbox.rs`
The actual enforcement layer for the Python sandbox.

- **`ALLOWED_MODULES`**: Whitelist of importable modules. Must be kept in sync with `tools/code_execution.rs`.
- **`SANDBOX_SETUP_CODE`**: Python code that:
  - Blocks dangerous builtins: `open`, `eval`, `exec`, `compile`, `input`, `breakpoint`, `globals`, `locals`, `vars`, `memoryview`.
  - Installs the restricted import hook.

## Limitations
- **RustPython**: Not all modules from the Python standard library are available due to `freeze-stdlib` limitations.
- **ModuleNotFoundError**: Some modules in `ALLOWED_MODULES` might still fail if they aren't compiled into the RustPython binary.

## Sync Guardrail
When modifying allowed/disallowed Python features:
1. Update `sandbox.rs` first.
2. Update `tools/code_execution.rs`.
3. Update system prompts in `src-tauri/src/lib.rs`.
4. Run tests: `cargo test -p python-sandbox`.

## Allowed Modules
```
math, json, random, re, datetime, collections, itertools, functools,
operator, string, textwrap, copy, types, typing, abc, numbers,
decimal, fractions, statistics, hashlib, base64, binascii, html
```
