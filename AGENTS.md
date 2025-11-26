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

### CSS Debugging - Global Overrides
- **Important**: When Tailwind classes don't seem to be working (e.g., `bg-white` not making an element white), **always check `src/index.css` first** for global CSS rules that may be overriding Tailwind.
- The `@layer base` section in `index.css` sets styles on `html`, `body`, and `#root` that take precedence over component-level Tailwind classes.
- **Common symptoms**: Background colors, fonts, or layouts not responding to Tailwind class changes.
- **Debugging steps**:
  1. Check `index.css` for global rules on `html`, `body`, `#root`, or `*` selectors
  2. Look for `background`, `color`, `font-family`, or layout properties that might conflict
  3. Either modify the global CSS or ensure your Tailwind classes have sufficient specificity

### Inline Styles vs Tailwind Classes
- **Prefer inline styles** when Tailwind utility classes are not being applied correctly due to CSS specificity issues.
- In this Tauri app, there are known specificity conflicts where Tailwind's `bg-*` classes get overridden by global CSS.
- **Use inline styles** for critical styling like background colors that must be applied reliably:
  ```jsx
  style={{ backgroundColor: '#e5e7eb' }}  // gray-200
  ```
- Reference Tailwind color values: gray-100=#f3f4f6, gray-200=#e5e7eb, gray-300=#d1d5db, white=#ffffff

### Debugging
- **Layout Debugger**: A built-in tool dumps detailed DOM dimensions to the console and backend terminal.
  - **Trigger**: Press `Ctrl+Shift+L` in the app.
  - **Implementation**: `debugLayout` function in `App.tsx`.
- **Backend Logging**: The `log_to_terminal` Tauri command is available to pipe frontend logs to the backend terminal for easier debugging.

## Backend Integration
- **Streaming**: Chat responses are streamed via the `chat-token` event, which appends text to the last assistant message in the store.
- **Commands**: Key Tauri commands include `get_models`, `set_model`, and `get_all_chats`.
