---
name: models-and-tools
description: Which chat models plugable-chat uses and how their tool calls are formatted, parsed and gated (Phi, Qwen, Foundry SDK, demo database). Use when changing default models, prompts, tool parsing, model switching, or debugging "the model answered but no tool ran".
---

# Models, tool calling and Foundry

- **Default `qwen3.5-4b`, fallback `phi-4-mini-instruct`** (never blocklisted) in
  `src-tauri/src/actors/foundry/service_manager.rs`. Startup selection order: saved settings model -> default -> fallback.
  Phi-4-mini answered all 7 Chicago questions; qwen3.5-4b needs its A10G re-run before it can stay the default (criterion:
  at least 6/7 within 3 minutes, otherwise ship Phi-4-mini).
- **Tool-call formats.** Phi and Qwen 2.5/3 use JSON in `<tool_call>{"name":..,"arguments":{..}}</tool_call>` (Hermes). **Qwen 3.5
  emits XML:** `<tool_call><function=sql_select><parameter=sql>...</parameter></function></tool_call>`, after a short `<think>`
  block (checked with llama.cpp and the Qwen3.5-4B GGUF: 93 tokens in 1.4 s on a Mac). The parser accepts both
  (`src-tauri/src/tool_parsing/hermes_parser.rs`), there is a Qwen family/XML tool format and prompt (`828ba91`), and a `<think>`
  block is shown as reasoning (`f05b51e`). Unparsed XML made the loop end silently after the planning text: that was the A10G
  "too slow" symptom, not slowness.
- **Tool gating.** Models say `sql`; the built-in is `sql_select` (aliased). An unknown tool now raises a visible warning
  instead of an empty reply. `sql_select` is blocked by the state machine until tables are known.
- **Chicago demo database** (`embedded-demo`, table `main.chicago_crimes`): served by the MCP Database Toolbox
  (`toolbox`, pinned 0.24.0, downloaded on demand). Launches with `PLUGABLE_ENABLE_DEMO_DB=true` index the schema in the
  background, and the launch prompt waits for it (else the model is told "no tables cached").
- **Foundry SDK.** Pinned `1.2.3` (winml on Windows only). Models the SDK cannot run are recorded in `incompatible_models`
  in `config.json` with the SDK key (`sdk-1.2.3`); the UI now says why and offers the model's CPU build. Known:
  qwen3.5-4b CUDA fails on Turing (T4); macOS `qwen3.5-4b-generic-gpu` failed on SDK 1.2.0 with a WebGPU validation error
  (fixed by 1.2.3 per its release note); the Mac CPU build `qwen3.5-4b-generic-cpu:3` works.
- **Open:** the launch prompt can run on the previously saved model before a `PLUGABLE_MODEL` override applies; Qwen tool prompt
  should match the model's native format everywhere; candidate follow-up tasks are in the session notes.
- **Local check on a Mac (rung 1b):** `foundry model download qwen3.5-4b-generic-cpu:3`, then run the app with the `PLUGABLE_*`
  env vars and read the log; back up and restore `~/Library/Application Support/plugable-chat/config.json`.
