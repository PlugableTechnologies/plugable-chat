import { formatVersionReport, type VersionInfo } from './version-info.ts';

export interface SupportDeps {
    invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
    writeClipboard: (text: string) => Promise<void>;
}

/**
 * Put everything support needs on the clipboard. The backend bundle has versions, the machine, the
 * GPU as Windows and the engine each see it, and the log tail. If the backend cannot build it, copy
 * what the screen showed plus the version and the reason, so the button never copies nothing.
 */
export async function copySupportDetails(shown: string, deps: SupportDeps): Promise<{ ok: boolean; text: string }> {
    let text: string;
    try {
        text = await deps.invoke<string>('get_support_bundle', { extra: shown });
    } catch (e) {
        let version = 'Plugable Chat version: unavailable';
        try {
            version = formatVersionReport(await deps.invoke<VersionInfo>('get_version_info'));
        } catch {
            // the version is best effort here: the point is to still copy something
        }
        text = [version, '', 'What the app showed:', shown, '', `(Full details could not be collected: ${e instanceof Error ? e.message : String(e)})`].join('\n');
    }
    try {
        await deps.writeClipboard(text);
        return { ok: true, text };
    } catch {
        return { ok: false, text };
    }
}
