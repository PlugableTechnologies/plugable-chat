/**
 * When the Databases tab's red "Schema refresh failed" alert should go away on its own.
 *
 * rc14 cleared it only when the user clicked Refresh again, so after the toolbox finished
 * installing and the automatic retry succeeded, the stale error stayed until the app restarted.
 */
export interface RefreshProgressEvent {
    source_name: string;
    is_complete: boolean;
    error: string | null;
}

/** A completed refresh without an error means that source is fine now. */
export function refreshSucceeded(e: RefreshProgressEvent): boolean {
    return e.is_complete && !e.error;
}

/** Keeps only the errors for sources that have not since refreshed successfully (matched by name). */
export function dropResolvedErrors<T extends { source_name: string }>(
    errors: Record<string, T>,
    e: RefreshProgressEvent,
): Record<string, T> {
    if (!refreshSucceeded(e)) return errors;
    // The automatic retry after an install reports one result for every source (empty name).
    if (e.source_name === '') return {};
    const next: Record<string, T> = {};
    for (const [id, err] of Object.entries(errors)) {
        if (err.source_name !== e.source_name) next[id] = err;
    }
    return next;
}

/** The toolbox finishing its download or install can fix a "toolbox not downloaded" failure. */
export function isToolboxBlockedError(message: string | null): boolean {
    if (!message) return false;
    const m = message.toLowerCase();
    return m.includes('toolbox not downloaded') || m.includes('toolbox failed to start') || m.includes('toolbox is not installed');
}
