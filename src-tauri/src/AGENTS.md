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
- The `chat` command (core agentic loop - too intertwined to extract cleanly)
- `get_system_prompt_preview` and `get_system_prompt_layers` (preview-specific logic)
- The `run()` function (Tauri app initialization)
- Cross-cutting internal helpers (e.g., auto-discovery, tool execution internals)

## Model-Specific Tool Calling Architecture
The system supports multiple model families with different tool calling behaviors. Model-specific handling flows through four layers:

### 1. Model Profiles (`model_profiles.rs`)
Defines `ModelFamily`, `ToolFormat`, and `ReasoningFormat` for each model.

### 2. Execution Parameters (`actors/foundry_actor.rs`)
`build_chat_request_body()` sets model-family-specific parameters (e.g., `max_tokens`, `temperature`, `top_k`).

### 3. System Prompt Building (`lib.rs`)
`build_system_prompt()` constructs tool instructions based on available tools.

### 4. Response Parsing (`tool_adapters.rs`)
`parse_tool_calls_for_model()` routes to the appropriate parser:
- `ToolFormat::OpenAI`
- `ToolFormat::Hermes`
- `ToolFormat::Granite`
- `ToolFormat::Gemini`
- `ToolFormat::TextBased` (fallback)

## Agentic Loop Processing (`lib.rs`)
The `run_agentic_loop()` function handles tool execution:
1. **Server Resolution**: Built-in tools vs MCP tools.
2. **Auto-Approval**: Check built-in auto-approval and MCP server configs.
3. **Execution**: Routing to `PythonActor`, `ToolRegistry`, or `McpHostActor`.
4. **Result Formatting**: `format_tool_result()` formats results per `ToolFormat`.

## Debugging Tool Calls
Key log prefixes to watch:
- `[FoundryActor]` - Request building
- `[AgenticLoop]` - Tool detection, resolution, execution
- `[parse_markdown_json_tool_calls]` - Fallback JSON parsing
- `[code_execution]` - Python code about to execute
- `[PythonActor]` - Sandbox execution details

## Tool Call Formats: enabled vs primary
- **Enabled**: Controls parsing/execution. Python blocks execute if Code Mode is enabled.
- **Primary**: Affects advertisement in the system prompt. Does not disable other enabled formats.

## TODOs
- Re-enable native tool payloads for Phi/Hermes once Foundry JSON schema validation accepts OpenAI tool definitions.
