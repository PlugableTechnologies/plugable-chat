// Models Plugable has tested end to end. Mirrors src-tauri/src/verified_models.rs; a test on each
// side checks that the two lists agree, so change both together.
//
// Verified = the release gate (Chicago questions on a real GPU box, docs/gpu-validation.md) ran it
// through the installed app. Anything else is "experimental": it may work, Plugable has not tested it.

export interface VerifiedModel {
    /** Catalog alias / name prefix, matched case-insensitively. */
    id: string;
    /** Catalog version numbers (the ":N" suffix) of the builds the gate ran. */
    testedVersions: string[];
}

export const VERIFIED_MODELS: VerifiedModel[] = [
    { id: 'qwen3.5-4b', testedVersions: ['4'] },
    { id: 'phi-4-mini-instruct', testedVersions: ['5'] },
];

export type ModelTrust = 'verified' | 'verified-new-build' | 'experimental';

export const EXPERIMENTAL_SECTION_TITLE = 'Experimental (not tested by Plugable)';
export const EXPERIMENTAL_WARNING =
    'Plugable has not tested these models. They may be slow, give poor answers, ignore tools, or fail to load.';
export const VERIFIED_SECTION_TITLE = 'Verified by Plugable';

function familyMatches(lower: string, family: string): boolean {
    if (!lower.startsWith(family)) return false;
    const rest = lower.slice(family.length);
    return rest === '' || rest.startsWith('-') || rest.startsWith(':');
}

function versionOf(lower: string): string | undefined {
    const i = lower.lastIndexOf(':');
    if (i < 0) return undefined;
    const v = lower.slice(i + 1);
    return v === '' ? undefined : v;
}

export function modelTrust(modelId: string): ModelTrust {
    const lower = modelId.trim().toLowerCase();
    const fam = VERIFIED_MODELS.find((v) => familyMatches(lower, v.id));
    if (!fam) return 'experimental';
    const ver = versionOf(lower);
    return ver !== undefined && !fam.testedVersions.includes(ver) ? 'verified-new-build' : 'verified';
}

export function isVerifiedModel(modelId: string): boolean {
    return modelTrust(modelId) !== 'experimental';
}

const TRUST_RANK: Record<ModelTrust, number> = { verified: 0, 'verified-new-build': 1, experimental: 2 };

/** Short note for a card or option, or null when there is nothing to warn about. */
export function trustNote(modelId: string): string | null {
    switch (modelTrust(modelId)) {
        case 'verified':
            return null;
        case 'verified-new-build':
            return 'A newer build than the one Plugable tested.';
        case 'experimental':
            return 'Not tested by Plugable.';
    }
}

/**
 * Split models into the verified list (default and fallback first, then as given) and the
 * experimental list. `idOf` returns whatever id the catalog/cache uses for the model.
 */
export function groupByTrust<T>(models: readonly T[], idOf: (m: T) => string): { verified: T[]; experimental: T[] } {
    const verified: T[] = [];
    const experimental: T[] = [];
    for (const m of models) {
        (isVerifiedModel(idOf(m)) ? verified : experimental).push(m);
    }
    // Stable: tested builds ahead of new builds, otherwise keep the caller's order.
    const indexed = verified.map((m, i) => ({ m, i }));
    indexed.sort((a, b) => TRUST_RANK[modelTrust(idOf(a.m))] - TRUST_RANK[modelTrust(idOf(b.m))] || a.i - b.i);
    return { verified: indexed.map((x) => x.m), experimental };
}
