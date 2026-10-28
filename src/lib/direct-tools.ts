// MCP tools that take one text field are offered in the model drop-down and called directly, with no local
// model in between. The backend decides which tools qualify (src-tauri/src/mcp_direct_tools.rs); this file is
// how the drop-down names, finds and orders them.

/** One drop-down entry, as the backend sends it. */
export interface DirectTool {
    server_id: string;
    server_name: string;
    tool_name: string;
    field_name: string;
    /** `server - tool`, shown as the option text. */
    label: string;
    description?: string | null;
}

/** The part of a tool that identifies it; this is what is remembered between launches. */
export interface DirectToolRef {
    server_id: string;
    tool_name: string;
}

/** Drop-down label: it names tools as well once any server offers one. */
export function modelDropdownLabel(tools: DirectTool[]): string {
    return tools.length > 0 ? 'Model / Tool:' : 'Model:';
}

const TOOL_VALUE_PREFIX = 'tool:';

/** The `<option>` value of a tool: never equal to a model id, which never starts with `tool:`. */
export function directToolOptionValue(ref: DirectToolRef): string {
    return `${TOOL_VALUE_PREFIX}${encodeURIComponent(ref.server_id)}:${encodeURIComponent(ref.tool_name)}`;
}

/** The tool an `<option>` value names, or null for a model id. */
export function parseDirectToolOptionValue(value: string): DirectToolRef | null {
    if (!value.startsWith(TOOL_VALUE_PREFIX)) return null;
    const [server, tool, ...rest] = value.slice(TOOL_VALUE_PREFIX.length).split(':');
    if (!server || !tool || rest.length > 0) return null;
    try {
        return { server_id: decodeURIComponent(server), tool_name: decodeURIComponent(tool) };
    } catch {
        return null;
    }
}

/** The chosen tool, when it is still offered; a remembered choice whose server is not connected yet gives null. */
export function activeDirectTool(tools: DirectTool[], chosen: DirectToolRef | null): DirectTool | null {
    if (!chosen) return null;
    return tools.find((t) => t.server_id === chosen.server_id && t.tool_name === chosen.tool_name) ?? null;
}

/** The offered tool whose drop-down text is `label` ("server - tool"); case and surrounding spaces are forgiven. */
export function toolFromLabel(tools: DirectTool[], label: string): DirectTool | null {
    const wanted = label.trim().toLowerCase();
    if (!wanted) return null;
    return tools.find((t) => t.label.trim().toLowerCase() === wanted) ?? null;
}

/**
 * A tool named by a label no connected server offers (yet): the label split on its last " - ". The backend then
 * reports "that tool is no longer connected" for the turn, so a mistyped name ends the run with a record instead of hanging.
 */
export function refFromLabel(label: string): DirectToolRef | null {
    const at = label.lastIndexOf(' - ');
    if (at <= 0) return null;
    const server = label.slice(0, at).trim();
    const tool = label.slice(at + 3).trim();
    return server && tool ? { server_id: server, tool_name: tool } : null;
}

/** The Settings tabs a launch can open (same ids as settings-store's `activeTab`; src-tauri/src/cli.rs checks them first). */
export const SETTINGS_TAB_IDS = ['models', 'databases', 'builtins', 'tools', 'system-prompt', 'interfaces', 'schemas', 'files', 'network'] as const;
export type SettingsTabId = (typeof SETTINGS_TAB_IDS)[number];

export function asSettingsTab(raw: string | null | undefined): SettingsTabId | null {
    return SETTINGS_TAB_IDS.find((t) => t === raw) ?? null;
}
