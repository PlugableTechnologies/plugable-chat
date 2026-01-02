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

### CLI Parity With UI
- Philosophy: **every end-user UI setting has a command-line argument equivalent** (clap/argparse). When adding a UI toggle/field, add a matching CLI flag and keep behaviors in sync.
- Key flags: `--system-prompt`, `--initial-prompt`, `--model`, `--tool-search`, `--python-execution`, `--python-tool-calling`, `--legacy-tool-call-format`, `--tool-call-enabled`, `--tool-call-primary`, `--tool-system-prompt`, `--mcp-server` (JSON or @file), `--tools` (allowlist).
- CLI overrides are ephemeral for the current launch (not persisted to the config file) but are visible via `get_launch_overrides` for the frontend to honor.

### Dynamic Port Addressing (CRITICAL)
- **Directive**: NEVER use fixed IP ports (e.g., `localhost:1234`, `127.0.0.1:8080`) in any strings, constants, or hardcoded URLs.
- **Reasoning**: Every server in the ecosystem, especially **Microsoft Foundry Local**, is dynamic. Ports are assigned at runtime and may change on every launch.
- **Action**: Always use dynamic port discovery, relative paths, or configuration-driven addressing. Check for `port` fields in server manifests or status payloads rather than assuming a default.

### GPU Memory & Model Eviction (CRITICAL)
- **Constraint**: Only ONE model can be loaded into GPU memory at a time. This includes:
  - **LLM models** (e.g., Phi-4-mini for chat)
  - **Embedding models** (e.g., BGE-Base-EN-v1.5 for RAG/search)
  - **Voice models** (future: speech-to-text, text-to-speech)
- **Silent Eviction**: When a new model is loaded, it **silently evicts** any previously loaded model. There is no error or warning—the old model simply becomes unavailable.
- **Implications**:
  1. **Don't pre-load competing models**: At startup, only load the CPU embedding model. The GPU embedding model should be loaded on-demand when embedding/caching is requested.
  2. **Re-warm after GPU operations**: After GPU embedding operations, explicitly call `RewarmCurrentModel` to reload the LLM into GPU memory.
  3. **Use CPU for chat-time search**: For search/tool lookups during chat turns, use the CPU embedding model to avoid evicting the pre-warmed LLM.
- **GPU vs CPU Model Usage**:
  | Operation | Model | Reason |
  |-----------|-------|--------|
  | RAG document embedding | GPU | Bulk indexing, not during chat |
  | Database schema caching | GPU | Bulk indexing, not during chat |
  | Schema search (during turn) | CPU | Avoid LLM eviction |
  | Tool search (during turn) | CPU | Avoid LLM eviction |
  | Column search (during turn) | CPU | Avoid LLM eviction |
- **Current Implementation**:
  - `FoundryActor` provides `GetGpuEmbeddingModel` for lazy-loading the GPU embedding model
  - `process_rag_documents` and `refresh_database_schemas` request the GPU model on-demand, then trigger LLM re-warm after completion
  - CPU embedding model is always available for search without GPU contention
