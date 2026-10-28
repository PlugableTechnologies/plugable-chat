/**
 * Settings > Network: optional proxy, Hugging Face mirror and offline model folder.
 *
 * The backend stores `network` in config.json with snake_case keys; the form edits plain strings.
 * Blank means "use the defaults", so it is saved as null.
 */

export interface NetworkSettings {
    https_proxy: string | null;
    hf_endpoint: string | null;
    offline_model_folder: string | null;
}

export interface NetworkForm {
    httpsProxy: string;
    hfEndpoint: string;
    offlineModelFolder: string;
}

/** One host's result from `test_network_connection`. */
export interface HostCheckRow {
    host: string;
    ok: boolean;
    status: number | null;
    millis: number | null;
    reason: string | null;
    /** What the address is for, so a failing row says what stops working. */
    purpose?: string;
}

export const EMPTY_NETWORK_FORM: NetworkForm = { httpsProxy: '', hfEndpoint: '', offlineModelFolder: '' };

export function networkSettingsToForm(settings: Partial<NetworkSettings> | null | undefined): NetworkForm {
    return {
        httpsProxy: settings?.https_proxy ?? '',
        hfEndpoint: settings?.hf_endpoint ?? '',
        offlineModelFolder: settings?.offline_model_folder ?? '',
    };
}

function blankToNull(value: string): string | null {
    const trimmed = value.trim();
    return trimmed === '' ? null : trimmed;
}

export function formToNetworkSettings(form: NetworkForm): NetworkSettings {
    const endpoint = blankToNull(form.hfEndpoint);
    return {
        https_proxy: blankToNull(form.httpsProxy),
        hf_endpoint: endpoint === null ? null : endpoint.replace(/\/+$/, ''),
        offline_model_folder: blankToNull(form.offlineModelFolder),
    };
}

export function networkFormIsDirty(saved: NetworkForm, current: NetworkForm): boolean {
    return JSON.stringify(formToNetworkSettings(saved)) !== JSON.stringify(formToNetworkSettings(current));
}

function checkAddress(label: string, value: string | null, schemes: string[]): string | null {
    if (value === null) return null;
    let url: URL;
    try {
        url = new URL(value);
    } catch {
        return `${label} must be a web address such as http://host:port.`;
    }
    if (!schemes.includes(url.protocol.replace(':', '')) || url.hostname === '') {
        return `${label} must start with ${schemes.map((s) => `${s}://`).join(' or ')}.`;
    }
    return null;
}

/** Problems the screen can show before asking the backend (which checks again, including that the folder exists). */
export function validateNetworkForm(form: NetworkForm): { httpsProxy?: string; hfEndpoint?: string } {
    const s = formToNetworkSettings(form);
    const errors: { httpsProxy?: string; hfEndpoint?: string } = {};
    const proxy = checkAddress('The proxy address', s.https_proxy, ['http', 'https', 'socks5', 'socks5h']);
    if (proxy) errors.httpsProxy = proxy;
    const mirror = checkAddress('The mirror address', s.hf_endpoint, ['http', 'https']);
    if (mirror) errors.hfEndpoint = mirror;
    return errors;
}

/** What "reachable" proves; shown with the results because a download can still fail after it. */
export const REACHABLE_MEANS = 'Reachable means the name resolved and a secure connection was made. It does not prove a download will succeed.';

/** One plain line per host: "huggingface.co: reachable (41 ms)" or "huggingface.co: blocked - <why>". */
export function describeHostRow(row: HostCheckRow): string {
    if (row.ok) {
        const timing = row.millis === null ? '' : `, ${row.millis} ms`;
        // The check asks for each site's home page, which many of them answer with a 4xx.
        const expected = row.status !== null && row.status >= 400 && row.status < 500 ? ' is expected here' : '';
        return `${row.host}: reachable (HTTP ${row.status ?? '?'}${expected}${timing})`;
    }
    return `${row.host}: not reachable - ${row.reason ?? 'unknown reason'}`;
}

export function summarizeHostRows(rows: HostCheckRow[]): { allOk: boolean; failed: number; lines: string[] } {
    const failed = rows.filter((r) => !r.ok).length;
    return { allOk: rows.length > 0 && failed === 0, failed, lines: rows.map(describeHostRow) };
}

export type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

export async function loadNetworkForm(invoke: InvokeFn): Promise<NetworkForm> {
    return networkSettingsToForm(await invoke<NetworkSettings>('get_network_settings'));
}

/** Saves and returns the form as the backend normalized it. The backend's message is thrown as an Error. */
export async function saveNetworkForm(form: NetworkForm, invoke: InvokeFn): Promise<NetworkForm> {
    try {
        const saved = await invoke<NetworkSettings>('save_network_settings', { network: formToNetworkSettings(form) });
        return networkSettingsToForm(saved);
    } catch (e) {
        throw new Error(typeof e === 'string' ? e : e instanceof Error ? e.message : 'The settings could not be saved.');
    }
}

/** Tests the values on the screen (saved or not): each host is tried through the typed proxy. */
export async function testNetworkForm(form: NetworkForm, invoke: InvokeFn): Promise<HostCheckRow[]> {
    return invoke<HostCheckRow[]>('test_network_connection', { network: formToNetworkSettings(form) });
}
