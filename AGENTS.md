# Project Architecture & Guardrails

## Tech Stack
- **Frontend**: React 19 (Vite), TypeScript
- **Desktop Wrapper**: Tauri v2
- **Styling**: Tailwind CSS v4
- **State Management**: Zustand

## Architectural Choices

### State Management & Event Listeners
- **Global Store**: `src/store/chat-store.ts` manages application state.
- **Tauri Events**: The store manually manages Tauri event listeners (`chat-token`, `chat-finished`) using a setup/cleanup pattern with **generation counters** (`listenerGeneration`).
  - **Guardrail**: Do not refactor `setupListeners`/`cleanupListeners` into simple `useEffect` calls without preserving the race-condition guards. The manual management ensures listeners are not duplicated or leaked during hot reloads or rapid component mounts.

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
