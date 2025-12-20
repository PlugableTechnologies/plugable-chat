# Project Architecture & Guardrails

## Tech Stack
- **Frontend**: React 19 (Vite), TypeScript
- **Desktop Wrapper**: Tauri v2
- **Styling**: Tailwind CSS v4
- **State Management**: Zustand
- **Package Manager**: Use **npm only**. Do not introduce or run pnpm/yarn/bun; keep dependency installs and scripts on npm to match existing setup.

## Release Monitoring
- Monitor upstream Foundry Local releases for features (e.g., tool calling) we should mirror in Plugable Chat. Check the changelog regularly: https://github.com/microsoft/Foundry-Local/releases

### Grep-Friendly Identifier Naming
- **Directive**: Prefer descriptive, tag-like names for all identifiers (functions, variables, types, props). Each segment should be greppable to find all related code paths.
- **Backend examples (Rust)**: `VectorActor` ➜ `ChatVectorStoreActor`; `perform_search` ➜ `search_chats_by_embedding`; `vector` ➜ `embedding_vector`; channels like `tx` ➜ `chat_vector_request_tx`.
- **Frontend examples (TS/React)**: `messages` ➜ `chatMessages`; `onSend` ➜ `onSendChatMessage`; `inputValue` ➜ `chatInputValue`; store guards `listenerGeneration` ➜ `listenerGenerationCounter`.
- **Do not rename** persisted schema fields, IPC channel names, or external protocol keys without migration/compat review; keep column names and event names stable unless explicitly migrating.

### Descriptive UI Selectors
- **Directive**: Give every major UI element a clear, descriptive class and/or id that reflects its role (e.g., `app-header-bar`, `chat-thread`, `sidebar-history-list`, `settings-modal`). Avoid anonymous wrappers like plain `div` with only utility classes.
- **Purpose**: Makes styling and QA selectors stable and greppable; reduces brittle nth-child targeting.
- **Scope**: Apply to structural containers (pages, panels, toolbars, lists, dialogs, input bars, status toasts). Inline atoms (icons, badges) can inherit parent tags.
- **Format**: Kebab-case, role-first naming; prefer class to id unless uniqueness is required.

## Architectural Choices

### State Management & Event Listeners
- **Global Store**: `src/store/chat-store.ts` manages application state.
- **Tauri Events**: The store manually manages Tauri event listeners (`chat-token`, `chat-finished`) using a setup/cleanup pattern with **generation counters** (`listenerGeneration`).
  - **Guardrail**: Do not refactor `setupListeners`/`cleanupListeners` into simple `useEffect` calls without preserving the race-condition guards. The manual management ensures listeners are not duplicated or leaked during hot reloads or rapid component mounts.

### CLI Parity With UI
- Philosophy: **every end-user UI setting has a command-line argument equivalent** (clap/argparse). When adding a UI toggle/field, add a matching CLI flag and keep behaviors in sync.
- Key flags: `--system-prompt`, `--initial-prompt`, `--model`, `--tool-search`, `--python-execution`, `--python-tool-calling`, `--legacy-tool-call-format`, `--tool-call-enabled`, `--tool-call-primary`, `--tool-system-prompt`, `--mcp-server` (JSON or @file), `--tools` (allowlist).
- CLI overrides are ephemeral for the current launch (not persisted to the config file) but are visible via `get_launch_overrides` for the frontend to honor.

### Styling & Layout
- **Layout System**: The app uses a `fixed inset-0` root container with `overflow-hidden`.
  - **Guardrail**: Do not add global scrollbars to `body` or `html`. Scrollbars should be contained within specific components (e.g., `ChatArea`).
- **Tailwind v4**: The project uses Tailwind v4.
  - **Critical**: Tailwind v4 uses `@import "tailwindcss";` — NOT the old v3 directives (`@tailwind base; @tailwind components; @tailwind utilities;`).
  - **Symptom**: If Tailwind classes silently have no effect but inline styles work, the import syntax is likely wrong.
  - **Config**: `tailwind.config.js` is optional in v4 (auto-detects content). The PostCSS plugin is `@tailwindcss/postcss` (not `tailwindcss`).
- **Markdown & Math**: `src/index.css` contains **hardcoded overrides** for `.prose` and `.katex` classes.
  - **Guardrail**: These overrides are critical for correct rendering of `\boxed{}` math expressions and specific light-mode aesthetics. Do not remove them unless replacing with an equivalent robust solution.

### CSS Debugging - Global Overrides
- **Important**: When Tailwind classes don't seem to be working (e.g., `bg-white` not making an element white), **always check `src/index.css` first** for global CSS rules that may be overriding Tailwind.
- The `@layer base` section in `index.css` sets styles on `html`, `body`, and `#root` that take precedence over component-level Tailwind classes.
- **Common symptoms**: Background colors, fonts, or layouts not responding to Tailwind class changes.
- **Debugging steps**:
  1. Check `index.css` for global rules on `html`, `body`, `#root`, or `*` selectors
  2. Look for `background`, `color`, `font-family`, or layout properties that might conflict
  3. Either modify the global CSS or ensure your Tailwind classes have sufficient specificity

### Debugging
- **Layout Debugger**: A built-in tool dumps detailed DOM dimensions to the console and backend terminal.
  - **Trigger**: Press `Ctrl+Shift+L` in the app.
  - **Implementation**: `debugLayout` function in `App.tsx`.
- **Backend Logging**: The `log_to_terminal` Tauri command is available to pipe frontend logs to the backend terminal for easier debugging.

## Backend Integration
- **Streaming**: Chat responses are streamed via the `chat-token` event, which appends text to the last assistant message in the store.
- **Commands**: Key Tauri commands include `get_models`, `set_model`, and `get_all_chats`.

### LanceDB Schema Management
- **Location**: Chat history is stored in LanceDB at `src-tauri/data/lancedb/chats.lance`.
- **Schema Definition**: The expected schema is defined in `get_expected_schema()` in `src-tauri/src/actors/vector_actor.rs`.
- **Schema Migration**: LanceDB does **not** automatically migrate schemas. If you add/remove columns:
  - The `setup_table()` function checks if the existing table's field count matches the expected schema.
  - On mismatch, it drops and recreates the table (losing existing data).
  - **Guardrail**: When modifying the schema (adding fields like `messages`, `pinned`, etc.), you must handle migration. The current approach is destructive—consider implementing proper data migration if preserving history is critical.
- **Common Symptom**: `RecordBatch` errors like `number of columns(6) must match number of fields(4)` indicate a schema mismatch between code and persisted table.
- **Debugging**: Check terminal logs for `VectorActor: Schema mismatch detected!` or `VectorActor: Table schema is up to date`.

## Model-Specific Tool Calling Architecture

The system supports multiple model families with different tool calling behaviors. Model-specific handling flows through four layers:

### Code Mode Philosophy (Advanced Tool Use)
- Code Mode is our implementation of Anthropic’s “advanced tool use”: discover tools on-demand with `tool_search`, orchestrate them programmatically in Python (`python_execution`), and include concise usage examples when useful.
- Goals: keep prompts lean for small models, defer non-critical tool schemas, and push multi-step/parallel work into Python to cut inference round-trips and context bloat.
- Guidance: when Code Mode is primary, remind the model to search first, then return one runnable Python program that calls discovered tools; built-ins (python_execution, tool_search) must be documented whenever enabled.
- Token discipline: examples are capped/optional, `defer_loading` plus compact prompt mode protects context for small-window models.

### 1. Model Profiles (`src-tauri/src/model_profiles.rs`)

Each model has a `ModelProfile` that defines its capabilities:
- **`ModelFamily`**: `GptOss`, `Phi`, `Gemma`, `Granite`, `Generic`
- **`ToolFormat`**: How the model outputs tool calls
  - `OpenAI`: Native `tool_calls` array in streaming response
  - `Hermes`: `<tool_call>{"name": "...", "arguments": {...}}</tool_call>` XML tags
  - `Granite`: `<function_call>...</function_call>` XML tags
  - `Gemini`: `function_call` JSON format
  - `TextBased`: Generic JSON detection (fallback)
- **`ReasoningFormat`**: `None`, `ThinkTags`, `ThinkingTags`, `ChannelBased`

Profile resolution: `resolve_profile(model_name)` matches model ID patterns to profiles.

### 2. Execution Parameters (`src-tauri/src/actors/foundry_actor.rs`)

The `build_chat_request_body()` function sets model-family-specific parameters:
- **GptOss**: `max_tokens=16384`, `temperature=0.7`, native tools
- **Phi**: Supports `reasoning_effort` parameter when reasoning model
- **Gemma**: `top_k=40` for controlled randomness
- **Granite**: `repetition_penalty=1.05`

Native tools are included in the request when `model_info.tool_calling == true`.

### 3. System Prompt Building (`src-tauri/src/lib.rs`)

`build_system_prompt()` constructs tool instructions based on available tools:
- Always documents `code_execution` (built-in Python sandbox)
- Adds `tool_search` when MCP servers are connected
- Includes tool calling format instructions (though smaller models may ignore them)

The system prompt tells models to use: `<tool_call>{"name": "TOOL_NAME", "arguments": {...}}</tool_call>`

### 4. Response Parsing (`src-tauri/src/tool_adapters.rs`)

`parse_tool_calls_for_model()` routes to the appropriate parser:

```
ToolFormat::OpenAI   → parse_hermes_tool_calls() (fallback to text)
ToolFormat::Hermes   → parse_hermes_tool_calls()
ToolFormat::Granite  → parse_granite_tool_calls()
ToolFormat::Gemini   → parse_gemini_tool_calls()
ToolFormat::TextBased → parse_granite_tool_calls() (Gemma uses <function_call>)
```

**Flexible field name extraction**:
- Tool name: `name` (standard) or `tool_name` (GPT-OSS legacy)
- Arguments: `arguments` (standard), `parameters` (Llama), or `tool_args` (GPT-OSS)

**Fallback chain** (each parser tries these in order):
1. Native format tags (`<tool_call>`, `<function_call>`, etc.)
2. Unclosed tags (streaming incomplete)
3. Braintrust format: `<function=name>{...}</function>` (Llama recipes)
4. Markdown JSON code blocks (` ```json ... ``` `) — for smaller models that ignore format instructions

**Native OpenAI streaming** (`delta.tool_calls`):
- Accumulated by `StreamingToolCalls` in `foundry_actor.rs`
- Converted to `<tool_call>` text at stream end for parser compatibility

### 5. Agentic Loop Processing (`src-tauri/src/lib.rs`)

The `run_agentic_loop()` function handles tool execution:

1. **Server Resolution**: 
   - Built-in tools (`code_execution`, `tool_search`) → `"builtin"` server
   - MCP tools → resolved via `resolve_server_for_tool()`

2. **Auto-Approval**:
   - Built-in tools: Always auto-approved
   - MCP tools: Check `server_configs[].auto_approve_tools`

3. **Execution**:
   - `code_execution` → `execute_code_execution()` → `PythonActor`
   - `tool_search` → `execute_tool_search()` → `ToolRegistry`
   - MCP tools → `execute_tool_internal()` → `McpHostActor`

4. **Result Formatting**: `format_tool_result()` formats results per `ToolFormat`

### Debugging Tool Calls

Key log prefixes to watch:
- `[FoundryActor]` - Request building, model capabilities
- `[AgenticLoop]` - Tool detection, server resolution, execution
- `[parse_markdown_json_tool_calls]` - Fallback JSON parsing
- `[code_execution]` - Python code about to execute
- `[PythonActor]` - Sandbox execution details

**Common Issues**:
- "Could not resolve server for tool" → Tool not recognized as built-in or MCP
- "No tool calls detected" → Model output doesn't match any parser format
- Logs appear to hang → Check `std::io::stdout().flush()` is called after prints

### Tool Call Formats: enabled vs primary
- Enabled formats control parsing/execution. If Code Mode is enabled (and python execution is allowed), Python blocks will execute even when another format is primary.
- Primary only affects what we advertise in the system prompt. It does not disable other enabled formats.
- To get Python execution, ensure Code Mode is enabled; setting it primary just changes prompting.

### TODOs
- Re-enable native tool payloads for Phi/Hermes once Foundry JSON schema validation accepts OpenAI tool definitions; currently disabled and using Hermes/text-based prompts to avoid 500s.

## Cursor for SQL (Database Toolbox)

The "Cursor for SQL" feature enables the agent to interact with SQL databases through a dedicated sidecar process, Google's **MCP Toolbox for Databases**. Our actor (`DatabaseToolboxActor`) manages the lifecycle of the toolbox process and exposes its capabilities as tools to the agent.

### Architecture
- **Process Management**: The `DatabaseToolboxActor` spawns and manages a separate binary (Google's "toolbox" binary) that handles actual database connections.
- **Communication**: The actor communicates with the Toolbox via standard MCP HTTP/SSE or stdio.
- **Tools**: The actor exposes capabilities as MCP tools, which are then wrapped by built-in tools (`sql_select`, `schema_search`) for the agent to use.

### Capabilities
1.  **Schema Discovery**:
    -   **Search**: `schema_search` tool allows semantic search over table schemas using embeddings.
    -   **Enumeration**: Can list schemas/datasets and tables for a given source.
    -   **Details**: Retrieves detailed table information including columns, types, and descriptions.
2.  **SQL Execution**:
    -   **Querying**: `sql_select` tool executes SQL queries against configured sources.
    -   **Safety**: Queries are executed with read-only permissions where possible (enforced by the database user configuration).
3.  **Connection Management**:
    -   **Testing**: Can test connections to configured sources.

### Supported Databases
The Toolbox supports the following databases (defined in `SupportedDatabaseKind`):
-   **BigQuery** (`bigquery`)
-   **PostgreSQL** (`postgres`)
-   **MySQL** (`mysql`)
-   **SQLite** (`sqlite`)
-   **Google Cloud Spanner** (`spanner`)

### Caching
-   **Table Schemas**: Discovered table schemas are cached on disk to reduce latency and database load.
-   **Embeddings**: Schema embeddings are generated and cached on disk to enable fast semantic search via `schema_search`.

## Python Sandbox Configuration Sync

The Python code execution sandbox has allowed/disallowed modules and builtins defined in **multiple locations** that must be kept in sync:

### Source of Truth Locations

1. **RustPython Sandbox** (`src-tauri/crates/python-sandbox/src/sandbox.rs`)
   - `ALLOWED_MODULES` constant - defines the whitelist of importable modules
   - `SANDBOX_SETUP_CODE` - Python code that blocks dangerous builtins (`open`, `eval`, `exec`, `compile`, `input`, `breakpoint`, `globals`, `locals`, `vars`, `memoryview`) and installs the restricted import hook

2. **Input Validation** (`src-tauri/src/tools/code_execution.rs`)
   - `ALLOWED_MODULES` constant - duplicated list for pre-execution validation
   - `validate_input()` - blocks patterns like `__import__`, `eval(`, `exec(`, `compile(`
   - `check_imports()` - validates imports before code reaches the sandbox

3. **Model Prompts** (`src-tauri/src/lib.rs`)
   - `build_system_prompt()` - tells models what Python capabilities are available
   - Should accurately describe allowed modules and restrictions

4. **Unit Tests** (`src-tauri/crates/python-sandbox/src/lib.rs`, `src-tauri/src/tools/code_execution.rs`)
   - Tests verify both allowed and blocked behavior
   - `test_all_allowed_modules` in code_execution.rs tests validation for every allowed module

### Guardrail: Keeping These in Sync

When modifying allowed/disallowed Python features:

1. **Update `sandbox.rs`** first - this is the actual enforcement layer
2. **Update `code_execution.rs`** - keep `ALLOWED_MODULES` identical to sandbox.rs
3. **Update system prompts** in `lib.rs` if the change affects what models should know
4. **Add/update tests** for both success and failure cases
5. **Run the full test suite**: `cargo test -p python-sandbox && cargo test code_execution`

### Current Allowed Modules

```
math, json, random, re, datetime, collections, itertools, functools,
operator, string, textwrap, copy, types, typing, abc, numbers,
decimal, fractions, statistics, hashlib, base64, binascii, html
```

**Note**: Not all modules may be available at runtime due to RustPython's `freeze-stdlib` limitations. The validation layer permits these imports, but the sandbox may return `ModuleNotFoundError` for modules not compiled into the RustPython binary.
