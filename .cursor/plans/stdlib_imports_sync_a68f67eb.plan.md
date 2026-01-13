---
name: stdlib_imports_sync
overview: Fix the critical bug where the Python runtime allowed modules set is out of sync with the Rust ALLOWED_MODULES constant. Generate the Python set from the Rust constant using runtime concatenation (WASM-compatible).
todos:
  - id: generate-set-func
    content: Create generate_allowed_modules_python_set() function in sandbox.rs
    status: completed
  - id: split-setup-code
    content: Split SANDBOX_SETUP_CODE into PART1 and PART2 constants
    status: completed
  - id: add-build-function
    content: Add build_sandbox_setup_code() function that concatenates parts
    status: completed
  - id: update-lib-rs
    content: Update lib.rs to use build_sandbox_setup_code() instead of SANDBOX_SETUP_CODE
    status: completed
  - id: test-imports
    content: Test that json, collections work and os/socket still fail
    status: completed
---

# Fix Stdlib Imports: Single Source of Truth (WASM-Compatible)

## Problem

Two separate allowed module lists exist and are out of sync:

- **Rust `ALLOWED_MODULES`** (source of truth with internal deps) - [sandbox.rs:531-599](src-tauri/crates/python-sandbox/src/sandbox.rs)
- **Python `_sandbox_allowed_modules`** (hardcoded subset) - [sandbox.rs:846-852](src-tauri/crates/python-sandbox/src/sandbox.rs)

The Python set controls actual runtime imports but is missing internal dependencies like `_collections_abc`, `_json`, etc.

## Solution

Use **runtime string concatenation** to build the setup code. This is:

- WASM-compatible (no lazy_static/once_cell needed)
- Simple to implement
- Minimal runtime overhead (one string allocation per execution)

## Implementation

### Step 1: Create function to generate Python set from Rust constant

In [sandbox.rs](src-tauri/crates/python-sandbox/src/sandbox.rs):

```rust
/// Generate Python code that creates the _sandbox_allowed_modules set
/// from the Rust ALLOWED_MODULES constant (single source of truth)
fn generate_allowed_modules_python_set() -> String {
    let mut modules: Vec<&str> = ALLOWED_MODULES.to_vec();
    modules.push("_sandbox");
    modules.push("builtins");
    
    let quoted: Vec<String> = modules.iter()
        .map(|m| format!("'{}'", m))
        .collect();
    
    format!("_sandbox_allowed_modules = {{{}}}", quoted.join(", "))
}
```

### Step 2: Split SANDBOX_SETUP_CODE into two parts

Split at the `_sandbox_allowed_modules = {...}` line:

```rust
/// Setup code BEFORE the allowed modules set
const SANDBOX_SETUP_PART1: &str = r##"
# Sandbox setup - import sandbox functions
from _sandbox import tool_call, get_tool_result, sandbox_print, sandbox_stderr

# Replace print with sandbox version  
import builtins
import sys
builtins.print = sandbox_print
# ... rest of setup until the set definition ...

# ============== Datetime Shim ==============
# ... datetime shim code ...
"##;

/// Setup code AFTER the allowed modules set (import restriction logic)
const SANDBOX_SETUP_PART2: &str = r##"
_original_import = builtins.__import__

def _restricted_import(name, globals=None, locals=None, fromlist=(), level=0):
    # ... import restriction logic ...

builtins.__import__ = _restricted_import
# ... cleanup ...
"##;
```

### Step 3: Add public function to build complete setup code

```rust
/// Build the complete sandbox setup code with dynamically generated allowed modules
pub fn build_sandbox_setup_code() -> String {
    format!(
        "{}\n\n{}\n\n{}",
        SANDBOX_SETUP_PART1,
        generate_allowed_modules_python_set(),
        SANDBOX_SETUP_PART2
    )
}
```

### Step 4: Update lib.rs to use the new function

In [lib.rs](src-tauri/crates/python-sandbox/src/lib.rs), change:

```rust
// Before
let setup_code = match vm.compile(
    SANDBOX_SETUP_CODE,
    Mode::Exec,
    "<sandbox_setup>".to_string(),
) { ... }

// After
let setup_code_str = build_sandbox_setup_code();
let setup_code = match vm.compile(
    &setup_code_str,
    Mode::Exec,
    "<sandbox_setup>".to_string(),
) { ... }
```

### Step 5: Update exports in sandbox.rs

```rust
// Remove or deprecate the old constant
// pub const SANDBOX_SETUP_CODE: &str = ...;

// Export the new function
pub use build_sandbox_setup_code;
```

## Files to Modify

- [src-tauri/crates/python-sandbox/src/sandbox.rs](src-tauri/crates/python-sandbox/src/sandbox.rs)
  - Add `generate_allowed_modules_python_set()` function
  - Split `SANDBOX_SETUP_CODE` into `SANDBOX_SETUP_PART1` and `SANDBOX_SETUP_PART2`
  - Add `build_sandbox_setup_code()` function

- [src-tauri/crates/python-sandbox/src/lib.rs](src-tauri/crates/python-sandbox/src/lib.rs)
  - Change import from `SANDBOX_SETUP_CODE` to `build_sandbox_setup_code`
  - Call function instead of using constant

- [src-tauri/src/tools/code_execution.rs](src-tauri/src/tools/code_execution.rs)
  - Note: Keep the duplicate `ALLOWED_MODULES` for now (used for pre-validation)
  - Consider importing from python-sandbox in future refactor

## Safety Preserved

The allowed modules list remains the same, just generated from a single source. Internal modules (`_` prefixed) are:

- Allowed at runtime (in `ALLOWED_MODULES`)
- Hidden from error messages (filtered in `_restricted_import`)
- Hidden from system prompt (separate `PYTHON_ALLOWED_IMPORTS` in system_prompt.rs)

## Testing

1. `import json` followed by `json.dumps({})` - should work
2. `from collections import defaultdict` - should work
3. `import os` - should fail with clean error (no `_` prefixed modules shown)
4. `import socket` - should fail
5. Verify error message only shows user-facing modules