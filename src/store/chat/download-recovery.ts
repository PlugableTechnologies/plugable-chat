/**
 * What the user can do when a model download fails or stalls. A failed download must never leave
 * only an error: Retry, a smaller model and "Copy details" are always offered (the first and last
 * are on every error row already; this decides the middle one).
 */
import { FALLBACK_MODEL } from './constants.ts';
import type { DownloadEntry } from './downloads.ts';
import { findCachedFallback, type CachedLike } from './fallback.ts';

/** No increase in the percentage for this long: offer the smaller model alongside Cancel. */
export const DOWNLOAD_STALL_AFTER_MS = 3 * 60 * 1000;

export function isDownloadStalled(e: DownloadEntry | undefined, now: number): boolean {
    if (!e || e.state !== 'active') return false;
    const since = e.lastProgressAt ?? e.startTime;
    return now - since >= DOWNLOAD_STALL_AFTER_MS;
}

/** The backend refuses a download when the drive is too full ("Not enough disk space for ..."). */
export function isDiskSpaceError(message: string | undefined): boolean {
    return !!message && /not enough disk space/i.test(message);
}

export type SmallerModelOffer =
    | { kind: 'switch'; modelId: string; label: string }
    | { kind: 'download'; model: string; label: string };

/**
 * The "Use smaller model" action for a failed or stalled download, or null when there is none to
 * offer (the failed model already is the fallback, so a second button would go nowhere).
 */
export function smallerModelOffer(failedId: string, cached: CachedLike[]): SmallerModelOffer | null {
    const failed = failedId.toLowerCase();
    if (failed.includes(FALLBACK_MODEL.toLowerCase())) return null;
    const hit = findCachedFallback(cached, FALLBACK_MODEL);
    if (hit) return { kind: 'switch', modelId: hit.model_id, label: hit.alias || hit.model_id };
    return { kind: 'download', model: FALLBACK_MODEL, label: FALLBACK_MODEL };
}

export function smallerModelButtonText(offer: SmallerModelOffer): string {
    return offer.kind === 'switch' ? `Use ${offer.label}` : 'Use smaller model';
}
