# Project Architecture & Guardrails

## Tech Stack
- **Frontend**: React 19 (Vite), TypeScript
- **Desktop Wrapper**: Tauri v2
- **Styling**: Tailwind CSS v4
- **State Management**: Zustand

## Architectural Choices

### State Management & Event Listeners
- **Global Store**: `src/store/chat-store.ts` manages application state.
- **Tauri Events**: The store manually manages Tauri event listeners (`chat-token`, `chat-finished`) using a setup/cleanup pattern with **generation counters** (`listenerGeneration`).
  - **Guardrail**: Do not refactor `setupListeners`/`cleanupListeners` into simple `useEffect` calls without preserving the race-condition guards. The manual management ensures listeners are not duplicated or leaked during hot reloads or rapid component mounts.

### Styling & Layout
- **Layout System**: The app uses a `fixed inset-0` root container with `overflow-hidden`.
  - **Guardrail**: Do not add global scrollbars to `body` or `html`. Scrollbars should be contained within specific components (e.g., `ChatArea`).
- **Tailwind v4**: The project uses Tailwind v4.
  - **Critical**: Tailwind v4 uses `@import "tailwindcss";` — NOT the old v3 directives (`@tailwind base; @tailwind components; @tailwind utilities;`).
  - **Symptom**: If Tailwind classes silently have no effect but inline styles work, the import syntax is likely wrong.
  - **Config**: `tailwind.config.js` is optional in v4 (auto-detects content). The PostCSS plugin is `@tailwindcss/postcss` (not `tailwindcss`).
- **Markdown & Math**: `src/index.css` contains **hardcoded overrides** for `.prose` and `.katex` classes.
  - **Guardrail**: These overrides are critical for correct rendering of `\boxed{}` math expressions and specific light-mode aesthetics. Do not remove them unless replacing with an equivalent robust solution.

### CSS Debugging - Global Overrides
- **Important**: When Tailwind classes don't seem to be working (e.g., `bg-white` not making an element white), **always check `src/index.css` first** for global CSS rules that may be overriding Tailwind.
- The `@layer base` section in `index.css` sets styles on `html`, `body`, and `#root` that take precedence over component-level Tailwind classes.
- **Common symptoms**: Background colors, fonts, or layouts not responding to Tailwind class changes.
- **Debugging steps**:
  1. Check `index.css` for global rules on `html`, `body`, `#root`, or `*` selectors
  2. Look for `background`, `color`, `font-family`, or layout properties that might conflict
  3. Either modify the global CSS or ensure your Tailwind classes have sufficient specificity

### Debugging
- **Layout Debugger**: A built-in tool dumps detailed DOM dimensions to the console and backend terminal.
  - **Trigger**: Press `Ctrl+Shift+L` in the app.
  - **Implementation**: `debugLayout` function in `App.tsx`.
- **Backend Logging**: The `log_to_terminal` Tauri command is available to pipe frontend logs to the backend terminal for easier debugging.

## Backend Integration
- **Streaming**: Chat responses are streamed via the `chat-token` event, which appends text to the last assistant message in the store.
- **Commands**: Key Tauri commands include `get_models`, `set_model`, and `get_all_chats`.

### LanceDB Schema Management
- **Location**: Chat history is stored in LanceDB at `src-tauri/data/lancedb/chats.lance`.
- **Schema Definition**: The expected schema is defined in `get_expected_schema()` in `src-tauri/src/actors/vector_actor.rs`.
- **Schema Migration**: LanceDB does **not** automatically migrate schemas. If you add/remove columns:
  - The `setup_table()` function checks if the existing table's field count matches the expected schema.
  - On mismatch, it drops and recreates the table (losing existing data).
  - **Guardrail**: When modifying the schema (adding fields like `messages`, `pinned`, etc.), you must handle migration. The current approach is destructive—consider implementing proper data migration if preserving history is critical.
- **Common Symptom**: `RecordBatch` errors like `number of columns(6) must match number of fields(4)` indicate a schema mismatch between code and persisted table.
- **Debugging**: Check terminal logs for `VectorActor: Schema mismatch detected!` or `VectorActor: Table schema is up to date`.
