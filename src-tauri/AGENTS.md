# Backend Architecture Overview

## State Machine Hierarchy (Cascading)

The system uses a three-tier cascading state machine architecture to manage complexity across different time scales (app lifetime, single turn, and tool execution loop). This decouples high-level user preferences from low-level execution details.

### Tier 1: Settings Layer (`SettingsStateMachine`)
- **Scope**: App lifecycle (updates when settings change).
- **Responsibility**: Computes the `OperationalMode` from raw `AppSettings` flags.
- **Key Output**: `OperationalMode` (e.g., `Conversational`, `SqlMode`, `CodeMode`, `HybridMode`).
- **Location**: `src-tauri/src/settings_state_machine.rs`

### Tier 2: Turn Layer (`AgenticStateMachine`)
- **Scope**: Single request turn (initialization to final response).
- **Responsibility**: Manages high-level flow based on the `OperationalMode`.
- **Key Output**: `AgenticState`.
- **Location**: `src-tauri/src/state_machine.rs`

### Tier 3: Mid-Turn Layer (`MidTurnStateMachine`)
- **Scope**: During execution (the "inner loop" of tool calling).
- **Responsibility**: Manages granular states during tool execution loops.
- **Location**: `src-tauri/src/mid_turn_state.rs`

## Backend Integration
- **Streaming**: Chat responses are streamed via the `chat-token` event, which appends text to the last assistant message in the store.
- **Commands**: Key Tauri commands include `get_models`, `set_model`, and `get_all_chats`.

## LanceDB Schema Management
- **Location**: Chat history is stored in LanceDB at `src-tauri/data/lancedb/chats.lance`.
- **Schema Migration**: LanceDB does **not** automatically migrate schemas. If you add/remove columns:
  - The `setup_table()` function checks if the existing table's field count matches the expected schema.
  - On mismatch, it drops and recreates the table (losing existing data).
  - **Guardrail**: When modifying the schema (adding fields like `messages`, `pinned`, etc.), you must handle migration. The current approach is destructive.
- **Common Symptom**: `RecordBatch` errors indicate a schema mismatch between code and persisted table.

## Cursor for SQL (Database Toolbox)
The "Cursor for SQL" feature enables the agent to interact with SQL databases through a dedicated sidecar process, Google's **MCP Toolbox for Databases**. Our actor (`DatabaseToolboxActor`) manages the lifecycle of the toolbox process and exposes its capabilities as tools to the agent.

### Architecture
- **Process Management**: The `DatabaseToolboxActor` spawns and manages a separate binary (Google's "toolbox" binary).
- **Communication**: The actor communicates with the Toolbox via standard MCP HTTP/SSE or stdio.
- **Tools**: The actor exposes capabilities as MCP tools, which are then wrapped by built-in tools (`sql_select`, `schema_search`).

### Supported Databases
The Toolbox supports: BigQuery, PostgreSQL, MySQL, SQLite, and Google Cloud Spanner.
