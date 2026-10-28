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
