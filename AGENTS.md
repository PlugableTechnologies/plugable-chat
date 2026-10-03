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

### Agentic Loop Philosophy: Cursor for SQL and RAG

Plugable Chat is designed as **"Cursor for SQL and RAG"** - the orchestration layer does the heavy lifting so small local models can succeed at complex tasks.

**Core Principles**:
1. **Plan and decompose** - Break complex queries into manageable steps
2. **Provide rich context** - Give the model exactly what it needs, when it needs it
3. **Recover from errors** - When something fails, don't give up; guide the model to fix it
4. **Learn from patterns** - Detect repeated failures and adapt the approach

**Implementation Pattern - Error Recovery**:
When a tool fails, don't just return the error. Re-inject the context the model needs to fix it:
- For SQL errors: Include the failed query, error message, AND the schema columns
- For Python errors: Include the traceback AND available imports/functions
- For MCP tool errors: Include the error AND the valid parameter schema

The goal is always: **don't make the model figure it out; tell it exactly what to do**.

See `build_sql_error_recovery_prompt()` in `system_prompt.rs` for the reference implementation.

## Testing order (for agents)

Test as low as possible: **local first, then EC2, then GitHub**, so few errors reach GitHub. Run
`scripts/preflight.sh` (about 1 minute) before every push and do not push on red. Use the paid GPU box
only for what needs real Windows or a GPU, and only after preflight passes. If a failure reaches a higher
rung than needed, add a check to the lower rung. Details, costs and past escapes:
[`docs/testing-ladder.md`](docs/testing-ladder.md); the same guidance is available as the `test-ladder` skill.

## Skills

Operational playbooks (what we planned, built, learned) live in `.claude/skills/`: `test-ladder`, `build-and-ci`,
`installer`, `gpu-validation`, `release-signing`, `models-and-tools`. Read the matching one before touching that area.

## Download page and user guide (for agents)

The top of `README.md`, `.github/release-notes-template.md` and `docs/user-guide.md` are what the public sees. Rules:
every claim in them needs a matching result in `docs/gpu-validation.md` (otherwise mark it "not yet tested" or leave it
out); the guide is UI-only (no command-line flags or terminal steps); screenshots come from real runs with no host names.
`scripts/ci/check-download-page.mjs` (in `preflight.sh` and CI) fails when the file names there stop matching what the
release produces. When a release is cut, re-check the status labels in the download table.

## GPU validation (for agents)

The app is tested on a real Windows + NVIDIA GPU box that exists only for the length of a run.
Read [`docs/gpu-validation.md`](docs/gpu-validation.md) first; it is the runbook, the record of what
went wrong before, and the list of open items. Rules that matter when you run it:

- **Cost and time limits.** A box costs about $0.53/hour (T4) or $1.0-1.4/hour (A10G). Every box gets
  an AWS-side hard stop (`MAX_RUN_MINUTES`, default 180) from `launch.sh`; do not remove it. **Do not
  start more than 3 GPU runs per task,** fix what CI (free) can show first, and stop and report after
  the same failure twice. Always finish with `infra/aws-gpu/teardown.sh` (it fails if anything billable
  is left). Never leave a box up while waiting for CI: launch it when the artifacts are nearly ready.
- **What you can rely on.** CI builds the unsigned debug installer and the compiled GPU tests
  (`gpu-tests-<sha>` is ready ~8 minutes after a push, the installer ~15 to 20). The box installs them,
  registers GPU providers itself, and can be asked the Chicago crimes questions with `ask.sh`; the
  expected answers are in `infra/aws-gpu/chicago-questions.json`.
- **Look at the screenshots.** The most important bugs so far (no GPU use, a broken `--initial-prompt`)
  were invisible to unit tests and found only by reading a screenshot of the installed app.
- **Drive the app with environment variables** (`PLUGABLE_MODEL`, `PLUGABLE_INITIAL_PROMPT`,
  `PLUGABLE_ENABLE_DEMO_DB`, `PLUGABLE_ALWAYS_ON_TABLES`), not command-line arguments with spaces.
- **Do not change** `.github/workflows/release.yml`, `scripts/sign-windows.mjs`,
  `scripts/verify-windows-signatures.ps1`, `.github/CODEOWNERS` or `src-tauri/tauri*.conf.json`
  without asking; they control what gets signed with the company certificate.
- **Default model:** `qwen3.5-4b`, fallback `phi-4-mini-instruct` (never blocklisted). `qwen3.5-4b`
  fails on an NVIDIA T4 (Turing) and is slow on an A10G; test both models when changing this.
