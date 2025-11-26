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
- **Markdown & Math**: `src/index.css` contains **hardcoded overrides** for `.prose` and `.katex` classes.
  - **Guardrail**: These overrides are critical for correct rendering of `\boxed{}` math expressions and specific light-mode aesthetics. Do not remove them unless replacing with an equivalent robust solution.

### Debugging
- **Layout Debugger**: A built-in tool dumps detailed DOM dimensions to the console and backend terminal.
  - **Trigger**: Press `Ctrl+Shift+L` in the app.
  - **Implementation**: `debugLayout` function in `App.tsx`.
- **Backend Logging**: The `log_to_terminal` Tauri command is available to pipe frontend logs to the backend terminal for easier debugging.

## Backend Integration
- **Streaming**: Chat responses are streamed via the `chat-token` event, which appends text to the last assistant message in the store.
- **Commands**: Key Tauri commands include `get_models`, `set_model`, and `get_all_chats`.
