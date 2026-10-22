/**
 * What to do when the backend says a model can't run and proposes another.
 *
 * rc14 downloaded the proposed model immediately, so a 2.2 GB model arrived with no request from
 * the user. Now a model that is already on disk is switched to, and anything that needs a
 * download waits for the user to say yes.
 */
export interface FallbackPayload {
    current_model: string;
    fallback_model: string;
    fallback_resolved_name?: string | null;
    fallback_size_bytes?: number | null;
}

export interface CachedLike {
    model_id: string;
    alias: string;
}

export type FallbackPlan =
    | { kind: 'switch'; modelId: string; name: string }
    | { kind: 'confirm-download'; model: string; label: string; sizeBytes: number | null }
    | { kind: 'none' };

export function findCachedFallback(cached: CachedLike[], fallback: string): CachedLike | undefined {
    const f = fallback.toLowerCase();
    return cached.find((m) => m.model_id.toLowerCase().includes(f) || m.alias.toLowerCase().includes(f));
}

export function planFallback(p: FallbackPayload, cached: CachedLike[], currentModel: string): FallbackPlan {
    const hit = findCachedFallback(cached, p.fallback_model);
    if (hit) {
        return hit.model_id === currentModel
            ? { kind: 'none' }
            : { kind: 'switch', modelId: hit.model_id, name: hit.alias || hit.model_id };
    }
    return {
        kind: 'confirm-download',
        model: p.fallback_model,
        label: p.fallback_resolved_name || p.fallback_model,
        sizeBytes: p.fallback_size_bytes ?? null,
    };
}

/** The question shown to the user. */
export function fallbackQuestion(failed: string, label: string, sizeBytes: number | null, format: (n: number) => string): string {
    const size = sizeBytes ? ` (${format(sizeBytes)})` : '';
    return `${failed} can't run on this PC. Download ${label}${size} to keep chatting?`;
}
