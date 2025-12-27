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
