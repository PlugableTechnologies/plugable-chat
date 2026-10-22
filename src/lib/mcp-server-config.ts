// Pure helpers for an MCP server's settings: the transport, the request timeout and "load tools immediately".
// No imports from the app, so `node --test` can run them (see mcp-server-config.test.ts).

export type McpTransport =
    | { type: 'stdio' }
    | { type: 'http'; url: string };

/** What a saved config may contain: 'sse' is how the screen once labelled the URL form; it never connected. */
export type SavedTransport = McpTransport | { type: 'sse'; url: string };

/** The longest request timeout the screen accepts: one day. A larger number is almost certainly a typo. */
export const MAX_REQUEST_TIMEOUT_SECS = 86_400;

/** What the app waits for one request when a server sets no timeout (kept in step with the Rust default). */
export const DEFAULT_REQUEST_TIMEOUT_SECS = 30;

/** Older saved configs say 'sse' for the URL form; read it as 'http'. Anything unrecognised is stdio. */
export function normalizeTransport(transport: SavedTransport | null | undefined): McpTransport {
    if (transport && (transport.type === 'http' || transport.type === 'sse')) {
        return { type: 'http', url: typeof transport.url === 'string' ? transport.url : '' };
    }
    return { type: 'stdio' };
}

/** null when the URL is usable, otherwise a short reason to show next to the field. Empty is "not yet", not an error. */
export function urlProblem(url: string): string | null {
    const trimmed = url.trim();
    if (!trimmed) return null;
    if (!/^https?:\/\//i.test(trimmed)) return 'The address must start with http:// or https://';
    try {
        const parsed = new URL(trimmed);
        if (!parsed.hostname) return 'The address has no host name';
    } catch {
        return 'This is not a valid address';
    }
    return null;
}

/** Text typed in the timeout box to the stored value: whole seconds from 1 up, empty or 0 meaning "use the default" (undefined). */
export function parseTimeoutInput(text: string): number | undefined {
    const trimmed = text.trim();
    if (!trimmed) return undefined;
    const n = Number(trimmed);
    if (!Number.isFinite(n)) return undefined;
    const whole = Math.floor(n);
    if (whole < 1) return undefined;
    return Math.min(whole, MAX_REQUEST_TIMEOUT_SECS);
}

/** The stored timeout as the box shows it: empty when none is set, so the placeholder shows the default. */
export function timeoutInputValue(secs: number | null | undefined): string {
    return secs && secs > 0 ? String(secs) : '';
}

/** "Load tools immediately" is the inverse of defer_tools, which defaults to true. */
export function loadsToolsImmediately(config: { defer_tools?: boolean | null }): boolean {
    return config.defer_tools === false;
}

export interface McpServerLike {
    id: string;
    name: string;
    transport?: SavedTransport | null;
    defer_tools?: boolean | null;
    request_timeout_secs?: number | null;
    python_name?: string;
}

/** Normalise a server before the screen shows it or the app saves it. Every other field, the timeout included, passes through untouched. */
export function normalizeMcpServer<T extends McpServerLike>(
    server: T,
    toPythonName: (name: string) => string,
): Omit<T, 'transport'> & { transport: McpTransport; defer_tools: boolean; python_name: string } {
    const isTestServer = server.id === 'mcp-test-server';
    const name = isTestServer ? 'mcp_test_server' : server.name;
    const { request_timeout_secs, ...rest } = server;
    const normalized = {
        ...rest,
        name,
        transport: normalizeTransport(server.transport),
        defer_tools: isTestServer ? false : (server.defer_tools ?? true),
        python_name: server.python_name ?? toPythonName(name || server.id),
    };
    // Unset stays absent (the backend does not write one back); a stored value is kept as is.
    const out = request_timeout_secs && request_timeout_secs > 0 ? { ...normalized, request_timeout_secs } : normalized;
    return out as unknown as Omit<T, 'transport'> & { transport: McpTransport; defer_tools: boolean; python_name: string };
}
