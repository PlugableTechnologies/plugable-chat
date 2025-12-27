# Core Backend Logic

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
