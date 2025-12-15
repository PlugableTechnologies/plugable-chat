# Tool Decision Tree

This document describes the decision logic for determining which tools are available, which formats to use in prompts, and how tools are presented to models.

## Architecture Simplifications

- **Built-in tools**: Tracked as a list (`enabled_builtins: Vec<String>`) rather than individual boolean flags
- **MCP tools**: Always deferred (discovered via `tool_search`). No `defer_tools` option.
- **Compact mode**: Removed. Max MCP tools in prompt is based on model size (default: 2).
- **Tool search**: Automatically executed before first model turn, but model can still call it.

## Tool Inclusion Decision Tree

```
Tool Included in Prompt?
├── Built-in Tool?
│   ├── Tool name in settings.enabled_builtins?
│   ├── filter.builtin_allowed(tool_name)?
│   └── Format compatibility check:
│       ├── python_execution: CodeMode in formats.enabled?
│       ├── tool_search: Always available if enabled
│       └── search_schemas/execute_sql: database_toolbox.has_enabled_sources?
│
└── MCP Tool?
    ├── server.enabled?
    ├── filter.server_allowed(server_id)?
    ├── filter.tool_allowed(server_id, tool_name)?
    └── Always deferred: Discovered via tool_search
        └── Included in prompt only after materialization
```

## Built-in Tools

The four built-in tools are:
1. `python_execution` - Python code execution sandbox
2. `tool_search` - Semantic search over MCP tools
3. `search_schemas` - Semantic search over database schemas
4. `execute_sql` - SQL query execution

**Decision logic per built-in:**

### python_execution
- Included if: `"python_execution" in enabled_builtins` AND `filter.builtin_allowed("python_execution")` AND `CodeMode in tool_call_formats.enabled`
- Purpose: Enables Code Mode (Python-driven tool orchestration)

### tool_search
- Included if: `"tool_search" in enabled_builtins` AND `filter.builtin_allowed("tool_search")` AND `has_deferred_mcp_tools`
- Purpose: Discovers deferred MCP tools. Always available for model to call even if not in initial prompt.

### search_schemas
- Included if: `"search_schemas" in enabled_builtins` AND `filter.builtin_allowed("search_schemas")` AND `database_toolbox.has_enabled_sources()`
- Purpose: Semantic search over database table schemas

### execute_sql
- Included if: `"execute_sql" in enabled_builtins` AND `filter.builtin_allowed("execute_sql")` AND `database_toolbox.has_enabled_sources()`
- Purpose: Execute SQL queries against configured databases

## MCP Tools

**All MCP tools are deferred by default.** They are discovered via `tool_search` and then materialized (made visible).

**Decision logic:**
1. Server must be `enabled` in settings
2. Server must pass `filter.server_allowed(server_id)`
3. Tool must pass `filter.tool_allowed(server_id, tool_name)`
4. Tool is registered with `defer_loading: true`
5. Tool becomes visible after `tool_search` discovers it and it's materialized

**Max tools in prompt:** Based on model size (default: 2 for small models). This limits how many MCP tool descriptions appear in the system prompt to avoid token bloat.

## Format Selection Decision Tree

```
Primary Format Selection
├── User preference: tool_call_formats.primary
├── Check availability:
│   ├── CodeMode: "python_execution" in enabled_builtins?
│   ├── Native: model.tool_calling == true?
│   └── Text-based (Hermes/Mistral/Pythonic/PureJson): Always available
└── Fallback: First enabled format that's available
```

**Format availability:**
- **Native**: Requires `model.tool_calling == true` AND `Native in tool_call_formats.enabled`
- **CodeMode**: Requires `"python_execution" in enabled_builtins` AND `CodeMode in tool_call_formats.enabled`
- **Text-based formats**: Always available if enabled (Hermes, Mistral, Pythonic, PureJson)

## Automatic Tool Search

Before the first model execution:
1. If `"tool_search" in enabled_builtins` AND there are deferred MCP tools:
2. Automatically run `tool_search` with the user's query as the search term
3. Materialize discovered tools
4. Include materialized tools in the prompt

The model can still call `tool_search` again during the conversation if needed.

## Prompt Construction Flow

1. **Resolve capabilities** using `ToolCapabilityResolver.resolve()`
   - Determines which built-ins are available
   - Determines which MCP tools are visible (materialized)
   - Selects primary format with fallback
   - Calculates max MCP tools for prompt (based on model size)

2. **Auto-run tool_search** (if enabled and deferred tools exist)
   - Uses user query to discover relevant tools
   - Materializes discovered tools

3. **Build system prompt**
   - Base prompt from settings
   - Format instructions (based on primary format)
   - Built-in tool descriptions (if available)
   - MCP tool descriptions (up to max, prioritized by relevance)

4. **Include tools in request**
   - Native tools: Include in `tools` array if Native format selected
   - Text-based: Include format instructions in system prompt
   - CodeMode: Include Python execution instructions

## Model-Specific Considerations

- **Small models** (e.g., < 7B parameters): Max 2 MCP tools in prompt
- **Medium models** (7B-30B): Max 5 MCP tools in prompt
- **Large models** (> 30B): Max 10 MCP tools in prompt

This prevents token bloat while still providing useful tool context.


