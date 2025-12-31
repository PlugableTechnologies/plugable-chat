# Frontend Architecture & Styling

## Descriptive UI Selectors
- **Directive**: Give every major UI element a clear, descriptive class and/or id that reflects its role (e.g., `app-header-bar`, `chat-thread`, `sidebar-history-list`, `settings-modal`). Avoid anonymous wrappers like plain `div` with only utility classes.
- **Purpose**: Makes styling and QA selectors stable and greppable; reduces brittle nth-child targeting.
- **Scope**: Apply to structural containers (pages, panels, toolbars, lists, dialogs, input bars, status toasts). Inline atoms (icons, badges) can inherit parent tags.
- **Format**: Kebab-case, role-first naming; prefer class to id unless uniqueness is required.

## Styling & Layout
- **Layout System**: The app uses a `fixed inset-0` root container with `overflow-hidden`.
  - **Guardrail**: Do not add global scrollbars to `body` or `html`. Scrollbars should be contained within specific components (e.g., `ChatArea`).
- **Tailwind v4**: The project uses Tailwind v4.
  - **Critical**: Tailwind v4 uses `@import "tailwindcss";` — NOT the old v3 directives (`@tailwind base; @tailwind components; @tailwind utilities;`).
  - **Symptom**: If Tailwind classes silently have no effect but inline styles work, the import syntax is likely wrong.
  - **Config**: `tailwind.config.js` is optional in v4 (auto-detects content). The PostCSS plugin is `@tailwindcss/postcss` (not `tailwindcss`).
- **Markdown & Math**: `src/index.css` contains **hardcoded overrides** for `.prose` and `.katex` classes.
  - **Guardrail**: These overrides are critical for correct rendering of `\boxed{}` math expressions and specific light-mode aesthetics. Do not remove them unless replacing with an equivalent robust solution.

## CSS Debugging - Global Overrides
- **Important**: When Tailwind classes don't seem to be working (e.g., `bg-white` not making an element white), **always check `src/index.css` first** for global CSS rules that may be overriding Tailwind.
- The `@layer base` section in `index.css` sets styles on `html`, `body`, and `#root` that take precedence over component-level Tailwind classes.
- **Common symptoms**: Background colors, fonts, or layouts not responding to Tailwind class changes.
- **Debugging steps**:
  1. Check `index.css` for global rules on `html`, `body`, `#root`, or `*` selectors
  2. Look for `background`, `color`, `font-family`, or layout properties that might conflict
  3. Either modify the global CSS or ensure your Tailwind classes have sufficient specificity

## Debugging
- **Layout Debugger**: A built-in tool dumps detailed DOM dimensions to the console and backend terminal.
  - **Trigger**: Press `Ctrl+Shift+L` in the app.
  - **Implementation**: `debugLayout` function in `App.tsx`.
- **Backend Logging**: The `log_to_terminal` Tauri command is available to pipe frontend logs to the backend terminal for easier debugging.

## Dynamic Port Addressing (CRITICAL)
- **Directive**: NEVER hardcode IP ports in API calls or websocket URLs.
- **Reasoning**: All local backend services and external tool servers (Foundry, MCP) are dynamic.
- **Action**: Use relative paths for Tauri IPC (`invoke`) or resolve ports from the application state/settings store.
