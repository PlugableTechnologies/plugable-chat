# State Management & Event Listeners

- **Global Store**: `src/store/chat-store.ts` manages application state.
- **Tauri Events**: The store manually manages Tauri event listeners (`chat-token`, `chat-finished`) using a setup/cleanup pattern with **generation counters** (`listenerGeneration`).
  - **Guardrail**: Do not refactor `setupListeners`/`cleanupListeners` into simple `useEffect` calls without preserving the race-condition guards. The manual management ensures listeners are not duplicated or leaked during hot reloads or rapid component mounts.

## Attachment Visibility Principle

**Invariant**: What the user sees as attached (pills in the UI) must match what is enabled and explained to the model.

### Per-Chat Attachments (show as pills, sent with each chat request)
- `attachedTools` → Tools explicitly attached for this chat (builtin or MCP)
- `attachedDatabaseTables` → Database tables attached for SQL queries
- `ragIndexedFiles` → Files indexed for RAG retrieval

### Always-On Attachments (show as locked pills, synced from settings)
- `alwaysOnTools` → Tools always enabled via settings
- `alwaysOnTables` → Tables always available via settings
- `alwaysOnRagPaths` → RAG paths always indexed via settings

### Chat Command Payload
When invoking the `chat` command, these attachments are passed to the backend:
```typescript
invoke('chat', {
  attachedFiles: storeState.ragIndexedFiles,
  attachedTables: storeState.attachedDatabaseTables.map(t => ({ ... })),
  attachedTools: storeState.attachedTools.map(t => t.key),
});
```

The backend's `compute_turn_config()` uses these to:
1. Add capabilities (e.g., `PythonExecution`, `SqlQuery`, `McpTools`) to `enabled_capabilities`
2. Transition to the appropriate state (e.g., `CodeExecution`, `SqlRetrieval`)
3. Include relevant guidance in the system prompt

**Guardrail**: If you add a new attachment type, ensure:
1. It's displayed as a pill in `ChatArea.tsx`
2. It's passed to the `chat` command
3. The backend's `compute_turn_config()` handles it
4. The state machine generates appropriate prompt guidance
