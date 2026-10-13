// Mirrors VersionInfo in src-tauri/src/version_info.rs. Both numbers matter to a tester: the release
// label says which build they were given, the commit says exactly what is in it.
export interface VersionInfo {
    appVersion: string;
    releaseLabel: string;
    gitSha: string;
    gitShaShort: string;
    dirty: boolean;
    buildTime: number;
}

export function formatVersionLine(v: VersionInfo | null | undefined): string {
    if (!v) return 'Version unavailable';
    return `${v.releaseLabel} (${v.gitShaShort}${v.dirty ? '+dirty' : ''})`;
}

// Text for a bug report. Never empty: if the backend did not answer, say so rather than send nothing.
export function formatVersionReport(v: VersionInfo | null | undefined): string {
    if (!v) return 'Plugable Chat version: unavailable (the app did not report it)';
    const built = v.buildTime > 0 ? new Date(v.buildTime * 1000).toISOString() : 'unknown';
    return [
        `Plugable Chat ${v.releaseLabel}`,
        `App version: ${v.appVersion}`,
        `Git commit: ${v.gitSha}${v.dirty ? ' (uncommitted changes)' : ''}`,
        `Built: ${built}`,
    ].join('\n');
}
