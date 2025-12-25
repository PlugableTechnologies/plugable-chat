# State Machine & Foundry Local Multi-Turn Conversation Analysis

This document analyzes how Plugable Chat's agentic state machine integrates with Foundry Local's conversation history and multi-turn conversation handling.

## Foundry Local's Conversation Model

According to the [Foundry Local REST API Reference](https://learn.microsoft.com/en-us/azure/ai-foundry/foundry-local/reference/reference-rest?view=foundry-classic), the `/v1/chat/completions` endpoint:

1. **Is fully compatible with OpenAI Chat Completions API**
2. **Is stateless** - each request sends the entire conversation history
3. **Message structure**:
   - `role`: Must be `system`, `user`, or `assistant`
   - `content`: The actual message text
4. **Optional `tools`** parameter for native tool calling (OpenAI format)

### Key API Properties

| Property | Description |
|----------|-------------|
| `model` | The specific model to use for completion |
| `messages` | The conversation history as a list of messages |
| `temperature` | Controls randomness (0-2) |
| `max_tokens` | Maximum tokens to generate |
| `stream` | When true, sends partial responses as SSE |
| `tools` | Optional tools for function calling |

## How Our State Machine Integrates

### 1. Message History Construction

Our code builds `full_history` in `lib.rs`:

```rust
// Build full history with system prompt at the beginning
let mut full_history = Vec::new();

// Add system prompt if we have one
full_history.push(ChatMessage { role: "system", content: system_prompt, ... });

// Add existing history (skip existing system messages to avoid duplicates)
for msg in history.iter() {
    if msg.role != "system" {
        full_history.push(msg.clone());
    }
}

// Add the new user message
full_history.push(ChatMessage { role: "user", content: message, ... });
```

The state machine generates the system prompt via `build_system_prompt()`:

```rust
pub fn build_system_prompt(&self) -> String {
    let mut sections: Vec<String> = vec![self.base_prompt.clone()];

    // Add factual grounding section (only once, at the top)
    if !matches!(self.current_state, AgenticState::Conversational) {
        sections.push(self.factual_grounding_section());
    }

    // Add state-specific prompt additions
    match &self.current_state {
        AgenticState::SqlRetrieval { discovered_tables, max_table_relevancy } => { ... }
        AgenticState::SqlResultCommentary { row_count, query_context, .. } => { ... }
        // etc.
    }
}
```

### 2. Mid-Turn System Prompt Updates

When the state machine transitions (e.g., after SQL execution), we update `full_history[0]`:

```rust
if should_continue {
    let new_prompt = state_machine.build_system_prompt();
    // Update the system message (first message with role "system") with the new prompt
    if !full_history.is_empty() && full_history[0].role == "system" {
        full_history[0].content = new_prompt.clone();
    }
}
```

**Key Insight**: Each iteration sends a **fresh** request to Foundry Local with the updated system prompt + accumulated message history. This is how our state machine dynamically tailors instructions mid-turn.

### 3. Tool Results as User Messages

Unlike OpenAI's native tool result format (with `tool_call_id`), we wrap tool results as user messages:

```rust
// Add all tool results as a single user message
let combined_results = tool_results.join("\n\n");
full_history.push(ChatMessage {
    role: "user".to_string(),
    content: combined_results,
    system_prompt: None,
});
```

### 4. Sending to Foundry

Before sending to Foundry, we strip local-only metadata:

```rust
// Strip any local-only metadata (like system_prompt) before sending to Foundry
let model_messages: Vec<ChatMessage> = full_history
    .iter()
    .map(|m| ChatMessage {
        role: m.role.clone(),
        content: m.content.clone(),
        system_prompt: None,
    })
    .collect();
```

## Compatibility Analysis

| Aspect | Foundry Local Support | Our Implementation | Status |
|--------|----------------------|-------------------|--------|
| Stateless messages array | ✅ | ✅ Full history sent each turn | **Compatible** |
| System/User/Assistant roles | ✅ | ✅ Used correctly | **Compatible** |
| Mid-turn system prompt changes | ✅ (just update messages[0]) | ✅ State machine updates it | **Compatible** |
| Native tool_calls (OpenAI format) | ✅ Optional `tools` param | ⚠️ Passed when available | **Partial** |
| Streaming response | ✅ `stream: true` | ✅ Used | **Compatible** |
| Tool result format | ✅ (as user message) | ⚠️ Text-wrapped, not native | **Works, not optimal** |

## State Machine Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        USER TURN START                               │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│  compute_initial_state()                                             │
│  - Evaluate RAG relevancy                                            │
│  - Evaluate schema search relevancy                                  │
│  - Check enabled capabilities                                        │
│  - Determine initial state (Conversational, RagRetrieval, etc.)     │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│  build_system_prompt()                                               │
│  - Base prompt + state-specific sections                             │
│  - Factual grounding (anti-hallucination)                           │
│  - Tool-specific instructions                                        │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│  SEND TO FOUNDRY LOCAL                                               │
│  POST /v1/chat/completions                                           │
│  { model, messages: [system, ...history, user], stream: true }      │
└─────────────────────────────────────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│  RECEIVE STREAMING RESPONSE                                          │
│  - Parse for tool calls                                              │
│  - Check if tool is allowed: state_machine.is_tool_allowed()        │
└─────────────────────────────────────────────────────────────────────┘
                                 │
              ┌──────────────────┴──────────────────┐
              │                                      │
              ▼                                      ▼
┌──────────────────────────┐          ┌──────────────────────────┐
│  NO TOOL CALLS           │          │  TOOL CALLS DETECTED     │
│  - Add to history        │          │  - Validate via state    │
│  - Emit finished         │          │  - Execute allowed tools │
│  - END                   │          │  - Trigger state event   │
└──────────────────────────┘          └──────────────────────────┘
                                                    │
                                                    ▼
                              ┌─────────────────────────────────────┐
                              │  handle_event(StateEvent)           │
                              │  - SqlExecuted → SqlResultCommentary│
                              │  - PythonExecuted → Handoff/Done    │
                              │  - ToolSearchCompleted → Discovered │
                              └─────────────────────────────────────┘
                                                    │
                                                    ▼
                              ┌─────────────────────────────────────┐
                              │  should_continue_loop()?            │
                              │  - SqlResultCommentary → YES        │
                              │  - CodeExecutionHandoff → YES       │
                              │  - ToolsDiscovered → YES            │
                              │  - Otherwise → NO                   │
                              └─────────────────────────────────────┘
                                                    │
                              ┌─────────────────────┴───────────────┐
                              │                                      │
                              ▼                                      ▼
                   ┌─────────────────┐                 ┌─────────────────┐
                   │  CONTINUE LOOP  │                 │  END TURN       │
                   │  - Regenerate   │                 │  - Emit finish  │
                   │    system prompt│                 │  - Save to DB   │
                   │  - Next iter    │                 └─────────────────┘
                   └─────────────────┘
                              │
                              └──────────────► (back to SEND TO FOUNDRY)
```

## Potential Issues & Recommendations

### Issue 1: Dynamic System Prompt Generation (FIXED)

**Problem**: The system prompt previously included hardcoded mentions of SQL and tool calling even when those capabilities weren't enabled:

```
## Capabilities
You are equipped with specialized tools to fetch real-time data, execute SQL queries...

## Factual Grounding
...MUST come from executing tools like `sql_select`...

## Tool Calling Format
When you need to use a tool, output ONLY: <tool_call>...
```

**Fix Applied**: The `build_system_prompt` function in `lib.rs` now uses two helper functions:

1. **`build_capabilities_section()`**: Dynamically describes only the enabled tools:
   - Only mentions SQL if `sql_select` or `schema_search` is enabled
   - Only mentions Python if `python_execution` is enabled
   - Only mentions MCP tools if servers are configured
   - Returns empty string if no tools are enabled

2. **`build_factual_grounding_section()`**: Tailors anti-hallucination guidance:
   - Only mentions `sql_select` if SQL is enabled
   - Provides lighter grounding for Python-only mode
   - Returns empty string if no data-retrieval tools are enabled

**Result**: The system prompt now accurately reflects only the tools that are actually available.

### Issue 2: Tool Result Message Format

**Current**: Tool results are sent as plain user messages with text formatting.

**Foundry Local expectation** (if native tool calling enabled): The API can accept `tool_calls` in the response and expects tool results in a specific format when using native tools.

**Impact**: Low for text-based tool formats (Hermes, Granite). Could be improved for native OpenAI format.

**Recommendation**: When using native tool calling mode, consider formatting tool results as:
```json
{
  "role": "tool",
  "tool_call_id": "<id>",
  "content": "<result>"
}
```

### Issue 3: System Prompt Drift in Long Conversations

**Current**: The system prompt is regenerated based on the current state, but the *history* of what the model was told in previous iterations is not preserved.

**Example Scenario**:
1. Turn 1: System prompt says "SQL tables available: orders, products"
2. Model runs SQL, state → `SqlResultCommentary`
3. New system prompt: "User sees the table, provide commentary"
4. Model produces commentary
5. On *next user turn*, system prompt reverts to base

**Impact**: The model may lose context about what it previously learned/did.

**Recommendation**: Consider:
- Preserving key context summaries across turns
- Adding a "context carried forward" section in the system prompt

### Issue 4: No Persistent Turn State

**Current**: State machine resets between user turns (`reset()` method).

**Foundry Local**: Doesn't track conversation state - it's stateless.

**Impact**: The state machine correctly handles intra-turn loops but loses inter-turn context.

**Recommendation**: This is architecturally correct. The state should be computed fresh each turn based on the user's new query + context. The message history provides continuity.

## Summary

Our state machine integrates **cleanly** with Foundry Local's stateless, OpenAI-compatible API:

1. **System prompt control** ✅ - State machine rebuilds prompt per state, injected into `messages[0]`
2. **Multi-turn history** ✅ - Full history accumulates and is sent each request
3. **Mid-turn iterations** ✅ - State transitions trigger prompt regeneration + continue loop
4. **Tool gating** ✅ - State machine blocks tools not allowed in current state

The main enhancement opportunity is supporting **native tool result message format** when models support native tool calling, which would provide cleaner semantics than wrapping everything in user messages.

## Future Enhancements

1. **Native Tool Result Format**: Implement proper `role: "tool"` messages for models that support native tool calling
2. **Context Summarization**: Add mechanism to carry forward key context between turns
3. **State Persistence**: Consider persisting state machine state for session recovery
4. **Token Budget Awareness**: Integrate with `/v1/chat/completions/tokenizer/encode/count` to manage context window

