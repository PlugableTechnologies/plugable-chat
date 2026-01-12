# Core Backend Logic

## Module Organization: Keeping `lib.rs` Lean

**Strategy**: Tauri commands are organized into domain-specific modules under `commands/` to prevent `lib.rs` from becoming a monolithic file. This improves maintainability, reduces merge conflicts, and makes code easier to navigate.

### Command Module Structure (`commands/`)
- `chat.rs` - Chat history and messaging commands
- `database.rs` - Database schema cache management
- `mcp.rs` - MCP server management and tool execution
- `model.rs` - Model loading, unloading, and catalog
- `rag.rs` - RAG document indexing and search
- `settings.rs` - Application settings and configuration
- `tool.rs` - Tool call detection, execution, and approval

### Guidelines
1. **New Tauri commands** should be added to the appropriate `commands/*.rs` module, NOT directly in `lib.rs`
2. **Re-export via `commands/mod.rs`**: Add `pub use module::*;` so commands are available via `commands::*`
3. **lib.rs imports all commands** via `use commands::*;` - this brings them into scope for the invoke handler
4. **Keep lib.rs focused on**: Module declarations, core agentic loop logic, app initialization (`run()`), and truly cross-cutting functionality
5. **Helper functions** used only by a command module should live in that module, not lib.rs

### What Stays in `lib.rs`
- Module declarations (`pub mod ...`)
- The `chat` command (Tauri command entry point)
- `get_system_prompt_preview` and `get_system_prompt_layers` (preview-specific logic)
- The `run()` function (Tauri app initialization)
- `tool_schema_to_mcp_tool()` helper for converting schemas

### Extracted Support Modules

**`agentic_loop.rs`** - Core agentic loop execution (~1,100 lines)
- `AgenticLoopConfig` - Configuration struct (consolidates 33 parameters)
- `AgenticLoopHandles` - Actor channels and shared state struct
- `AgenticLoopAction` - Result of action detection (ToolCalls vs Final)
- `detect_agentic_loop_action()` - Determine if response contains tool calls
- `run_agentic_loop()` - Main loop: call model → detect tool calls → execute → repeat
- `execute_builtin_tool_call()` - Dispatch to tool_search/python/schema/sql
- Key constants: `MAX_LOOP_ITERATIONS`

**`auto_discovery.rs`** - Auto-discovery before first turn
- `AutoDiscoveryContext` - Container for search results
- `auto_tool_search_for_prompt()` - Semantic tool search
- `auto_schema_search_for_prompt()` - Database schema search
- `perform_auto_discovery_for_prompt()` - Combined discovery

**`tool_execution.rs`** - Tool dispatch and execution
- `dispatch_tool_call_to_executor()` - Execute MCP tool calls
- `resolve_mcp_server_for_tool()` - Find server for unknown tool
- `execute_tool_search()` - Built-in tool_search execution
- `execute_python_code()` - Built-in python_execution execution
- `PYTHON_EXECUTION_TOOL_TYPE` - Tool type identifier constant

**`message_builders.rs`** - Chat message construction
- `create_assistant_message_with_tool_calls()` - Build assistant message
- `create_native_tool_result_message()` - Build tool result message
- `should_use_native_tool_results()` - Check if native format applies

**`python_helpers.rs`** - Python code processing
- `parse_python_execution_args()` - Parse tool arguments
- `fix_python_indentation()` - Fix missing indentation
- `strip_unsupported_python()` - Remove await keywords
- `is_valid_python_syntax()` - Syntax validation
- `reconstruct_sql_from_malformed_args()` - SQL recovery

## Model-Specific Tool Calling Architecture
The system supports multiple model families with different tool calling behaviors. Model-specific handling flows through four layers:

### 1. Model Profiles (`model_profiles.rs`)
Defines `ModelFamily`, `ToolFormat`, and `ReasoningFormat` for each model.

### 2. Execution Parameters (`actors/foundry/request_builder.rs`)
`build_foundry_chat_request_body()` sets model-family-specific parameters (e.g., `max_tokens`, `temperature`, `top_k`).

### 3. System Prompt Building (`lib.rs`)
`build_system_prompt()` constructs tool instructions based on available tools.

### 4. Response Parsing (`tool_parsing/`)
The `tool_parsing` module provides format-specific parsers for different model families:

**Module Structure:**
- `mod.rs` - Main exports, `format_tools_for_model()`, `parse_tool_calls_for_model_profile()`
- `hermes_parser.rs` - `parse_hermes_tool_calls()` for Phi, Qwen
- `harmony_parser.rs` - `parse_harmony_tool_calls()` for GPT-OSS
- `gemini_parser.rs` - `parse_gemini_tool_calls()` for Gemini
- `granite_parser.rs` - `parse_granite_tool_calls()` for Granite/Gemma
- `tagged_parser.rs` - `parse_tagged_tool_calls()` for Mistral-style [TOOL_CALLS]
- `pythonic_parser.rs` - `parse_pythonic_tool_calls()` for Python-style function calls
- `json_parser.rs` - `parse_pure_json_tool_calls()` for bare JSON
- `markdown_json_parser.rs` - `parse_markdown_json_tool_calls()` for ```json blocks
- `braintrust_parser.rs` - `parse_braintrust_function_calls()` for Llama
- `python_detector.rs` - `detect_python_code()` for Code Mode
- `result_formatter.rs` - `format_tool_result()` for response formatting
- `json_fixer.rs` - `repair_malformed_json()`, `parse_json_lenient()`
- `common.rs` - Shared utilities like `parse_combined_tool_name()`

**Supported Formats:**
- `ToolFormat::OpenAI` - Standard OpenAI tool_calls
- `ToolFormat::Hermes` - `<tool_call>JSON</tool_call>` XML format
- `ToolFormat::Harmony` - `<|channel|>commentary to=tool...` tokens
- `ToolFormat::Granite` - `<function_call>XML</function_call>` format
- `ToolFormat::Gemini` - function_call in response
- `ToolFormat::TextBased` - Fallback with multiple parser attempts

## Agentic Loop Processing (`agentic_loop.rs`)
The `run_agentic_loop()` function handles tool execution in a structured loop:

### Entry Point
The `chat()` command in `lib.rs` constructs `AgenticLoopConfig` and `AgenticLoopHandles`,
then spawns `run_agentic_loop()` as an async task.

### Config Structs
- `AgenticLoopConfig` - Behavior parameters (chat_id, model_name, format_config, etc.)
- `AgenticLoopHandles` - Actor channels (foundry_tx, mcp_host_tx, python_tx, etc.)

### Loop Flow
1. **Server Resolution**: Built-in tools vs MCP tools.
2. **Auto-Approval**: Check built-in auto-approval and MCP server configs.
3. **Execution**: `execute_builtin_tool_call()` or `dispatch_tool_call_to_executor()`.
4. **Result Formatting**: `format_tool_result()` formats results per `ToolFormat`.
5. **State Transitions**: `state_machine.handle_event()` for post-tool transitions.
6. **Error Recovery**: For SQL errors, injects schema context via `build_sql_error_recovery_prompt()`.

## Error Recovery Pattern

When tool execution fails, the agentic loop helps the model recover rather than giving up. This is the "Cursor for SQL" approach: the orchestration layer does the heavy lifting.

### The Pattern
1. **Detect error** - Tool returns `is_error: true`
2. **Extract context** - Get schema/parameters from state machine via `get_compact_schema_context()`
3. **Build recovery prompt** - Use `build_sql_error_recovery_prompt()` or equivalent
4. **Inject into tool response** - Model sees error + context + clear next action

### Key Functions
- `system_prompt::build_sql_error_recovery_prompt()` - SQL-specific recovery with schema injection
- `state_machine::get_compact_schema_context()` - Extract compact schema for error prompts
- `tool_parsing::format_tool_result()` - Routes to appropriate recovery builder based on tool type

### Why This Matters
Small models (Phi-4-mini, Llama 3.2) don't look back in context when they see an error. If we just say "check the schema above," they repeat the same mistake. By re-injecting the schema directly into the error response, we give them everything they need in their immediate context.

### Tests
- `test_sql_error_recovery_with_schema_injection` - Integration test that validates recovery
- `test_get_compact_schema_context` - Unit test for schema extraction

## Debugging Tool Calls
Key log prefixes to watch:
- `[FoundryActor]` - Request building and streaming
- `[AgenticLoop]` - Loop iterations, tool detection, resolution, execution, state transitions
- `[parse_markdown_json_tool_calls]` - Fallback JSON parsing
- `[code_execution]` - Python code about to execute
- `[PythonActor]` - Sandbox execution details
- `[Chat]` - High-level chat command flow

## Tool Call Formats: enabled vs primary
- **Enabled**: Controls parsing/execution. Python blocks execute if Code Mode is enabled.
- **Primary**: Affects advertisement in the system prompt. Does not disable other enabled formats.

## Dynamic Port Addressing (CRITICAL)
- **Invariant**: Hardcoded IP ports are forbidden.
- **Guideline**: Servers like Microsoft Foundry Local and MCP hosts use dynamic port allocation. All backend logic must resolve the current port from process output, registry files, or initialization handshake.
- **Prohibited**: `let url = "http://localhost:8080/..."`
- **Preferred**: `let url = format!("http://localhost:{}/...", current_foundry_port)`

## Attachment Visibility Principle (CRITICAL)

**Invariant**: The user's visible attachments (pills in the UI) must exactly match what is enabled and explained in the system prompt.

### The Rule
1. **If visible as a pill → MUST be enabled and explained in the system prompt**
   - User attaches `python_execution` tool → Python guidance appears in prompt
   - User attaches a database table → SQL guidance + schema appear in prompt
   - User attaches an MCP tool → Tool is available and documented in prompt
   - User attaches a file → RAG context is retrieved and included

2. **If NOT visible as a pill → MUST NOT be mentioned in the system prompt**
   - No SQL tables attached → No SQL-specific guidance (even if `sql_select_enabled=true`)
   - No Python tool attached → No Python sandbox guidance (even if capability exists)

### Implementation
- **Frontend** (`chat-store.ts`): Pills are rendered from `attachedTools`, `attachedDatabaseTables`, `ragIndexedFiles`
- **Backend** (`state_machine.rs`): `compute_turn_config()` updates `enabled_capabilities` based on attached items
- **Prompt Generation** (`build_system_prompt()`): Only includes guidance for capabilities in `enabled_capabilities`

### Common Bug Pattern
If a user attaches something (visible pill) but the model doesn't receive guidance:
1. Check if `compute_turn_config()` adds the capability to `enabled_capabilities`
2. Check if the state transition matches the attachment type
3. Check if `build_system_prompt_sections()` includes the relevant section

### Tests
- `test_turn_attached_python_enables_code_execution`
- `test_turn_attached_mcp_tool_enables_tool_orchestration`
- `test_turn_attached_table_enables_sql_mode`

## TODOs
- Re-enable native tool payloads for Phi/Hermes once Foundry JSON schema validation accepts OpenAI tool definitions.
