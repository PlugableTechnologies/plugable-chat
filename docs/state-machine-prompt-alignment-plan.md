# State Machine System Prompt Alignment Plan

## Problem Statement

The system prompt is currently built from two separate sources:
1. **`lib.rs::build_system_prompt()`** - Used for the initial prompt, with many conditional checks
2. **`state_machine.build_system_prompt()`** - Used for mid-turn regeneration

This creates risk of drift between what we tell the model and what we actually allow/execute.

## Current Data Flow

```
send_chat_message()
    │
    ├─► ToolCapabilityResolver.resolve()
    │       └─► ResolvedToolCapabilities { available_builtins, ... }
    │
    ├─► lib.rs::build_system_prompt()  ← SEPARATE from state machine
    │       └─► Many conditional checks in collect_tool_prompt_additions()
    │
    └─► run_agentic_loop()
            │
            └─► AgenticStateMachine::new()  ← Created AFTER prompt is built
                    └─► state_machine.build_system_prompt()  ← ONLY for mid-turn
```

## Proposed Architecture

### Phase 1: Extend State Machine with Prompt Context

The `AgenticStateMachine` needs additional context to fully replace lib.rs prompt building:

```rust
pub struct AgenticStateMachine {
    // Existing fields
    current_state: AgenticState,
    enabled_capabilities: HashSet<Capability>,
    thresholds: RelevancyThresholds,
    state_history: Vec<AgenticState>,
    base_prompt: String,
    
    // NEW: Prompt context
    mcp_tool_context: McpToolContext,
    tool_call_format: ToolCallFormatName,
    custom_tool_prompts: HashMap<String, String>,
}

pub struct McpToolContext {
    /// Active MCP tools (can be called immediately)
    pub active_tools: Vec<(String, Vec<McpTool>)>,
    /// Deferred MCP tools (require tool_search discovery)
    pub deferred_tools: Vec<(String, Vec<McpTool>)>,
    /// Server configurations (for env vars, auto-approve status)
    pub server_configs: Vec<McpServerConfig>,
}
```

### Phase 2: Create State Machine Before Prompt Building

```rust
// In send_chat_message(), BEFORE building the system prompt:

// 1. Create state machine with full context
let state_machine = AgenticStateMachine::new_with_context(
    &settings,
    &tool_filter,
    thresholds,
    base_prompt,
    McpToolContext {
        active_tools: active_mcp_tools,
        deferred_tools: deferred_mcp_tools,
        server_configs: server_configs.clone(),
    },
    primary_format,
    tool_system_prompts,
);

// 2. Compute initial state based on context
state_machine.compute_initial_state(
    rag_relevancy,
    schema_relevancy,
    discovered_tables,
    rag_chunks,
);

// 3. Build prompt FROM state machine (single source of truth)
let system_prompt = state_machine.build_system_prompt();
```

### Phase 3: Unified Prompt Building in State Machine

Move all prompt logic into `AgenticStateMachine::build_system_prompt()`:

```rust
impl AgenticStateMachine {
    pub fn build_system_prompt(&self) -> String {
        let mut sections = vec![self.base_prompt.clone()];
        
        // 1. Capabilities section (from enabled_capabilities)
        if let Some(caps) = self.build_capabilities_section() {
            sections.push(caps);
        }
        
        // 2. Factual grounding (only if data tools enabled)
        if self.has_data_retrieval_tools() {
            sections.push(self.build_factual_grounding());
        }
        
        // 3. State-specific context
        sections.push(self.build_state_context());
        
        // 4. Tool format instructions (from tool_call_format)
        if let Some(format) = self.build_format_instructions() {
            sections.push(format);
        }
        
        // 5. MCP tool descriptions (from mcp_tool_context)
        if let Some(tools) = self.build_mcp_tool_section() {
            sections.push(tools);
        }
        
        sections.join("\n\n")
    }
}
```

### Phase 4: Deprecate lib.rs Prompt Functions

1. Mark `lib.rs::build_system_prompt()` as `#[deprecated]`
2. Mark `collect_tool_prompt_additions()` as `#[deprecated]`
3. Remove after migration is complete

## Benefits

1. **Single Source of Truth**: The state machine controls both what we tell the model AND what we allow
2. **Aligned Execution**: `is_tool_allowed()` and prompt content come from the same state
3. **Easier Testing**: State transitions can be unit tested with predictable prompts
4. **Reduced Drift Risk**: No more two places to update when adding capabilities

## Migration Path

1. ✅ Phase 1a: Add `McpToolContext` to state machine (non-breaking)
2. ✅ Phase 1b: Add `tool_call_format` and `custom_tool_prompts` to state machine
3. ✅ Phase 2: Create state machine before prompt building in `send_chat_message()`
4. ✅ Phase 3: Move prompt logic from lib.rs to state machine
5. ✅ Phase 4: Deprecate old prompt functions

## Implementation Checklist (COMPLETED 2024-12-25)

- [x] Add `McpToolContext` struct to `agentic_state.rs`
- [x] Add `McpToolInfo` and `McpServerInfo` helper types
- [x] Add `PromptContext` struct for unified context passing
- [x] Add `from_tool_lists()` constructor for `McpToolContext`
- [x] Add new fields to `AgenticStateMachine` (mcp_context, tool_call_format, etc.)
- [x] Add `new_with_context()` constructor to state machine
- [x] Move capability section building to `build_capabilities_section()` in state machine
- [x] Move factual grounding building to `build_factual_grounding_section()` in state machine
- [x] Move tool format instructions to `build_format_instructions()` in state machine
- [x] Move MCP tool section building to `build_mcp_tool_section()` in state machine
- [x] Add `build_python_section()` for code mode
- [x] Update `send_chat_message()` to create state machine BEFORE building prompt
- [x] Use `state_machine.build_system_prompt()` for initial prompt
- [x] Update `run_agentic_loop()` signature to receive state machine as parameter
- [x] Remove internal state machine creation in `run_agentic_loop()`
- [x] Deprecate old `lib.rs::build_system_prompt()` and related functions
- [ ] Add comprehensive tests for prompt generation from various states (future)

