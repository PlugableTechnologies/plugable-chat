// Mirrors GpuAssessment in src-tauri/src/gpu_assessment.rs. The backend decides what is true; this
// file only decides how to show it, so "No GPU models are available" can never again stand in for
// four different situations.

export type GpuStateKind =
    | 'checking'
    | 'ready'
    | 'no_hardware'
    | 'hardware_not_used'
    | 'accelerated_no_models'
    | 'catalog_failed'
    | 'engine_unavailable'
    | 'scan_failed';

export interface GpuAssessment {
    state: GpuStateKind;
    headline: string;
    detail: string[];
    hints: string[];
    retryMayHelp: boolean;
}

export type CardTone = 'none' | 'info' | 'amber' | 'red';

/** How loudly to show the state. Ready and "still checking" show nothing; no GPU at all is information, not a fault. */
export function cardTone(a: GpuAssessment | null | undefined): CardTone {
    if (!a) return 'none';
    switch (a.state) {
        case 'ready':
        case 'checking':
            return 'none';
        case 'no_hardware':
            return 'info';
        case 'catalog_failed':
        case 'engine_unavailable':
            return 'red';
        default:
            return 'amber';
    }
}

/** The text a person reads: headline, what was observed, what to try. */
export function cardLines(a: GpuAssessment): string[] {
    return [a.headline, ...a.detail, ...a.hints.map((h) => `Try: ${h}`)];
}

/** Plain text for the card when it is copied for support. */
export function cardText(a: GpuAssessment): string {
    return cardLines(a).join('\n');
}

interface CatalogEntry {
    name: string;
    alias: string;
    runtime: { deviceType: string };
    incompatible?: boolean;
}

/**
 * When the app is running a CPU build and the GPU has since become usable (a retry succeeded, an
 * external GPU was connected late), the GPU build of the same model to offer. Null otherwise.
 */
export function findGpuUpgrade(
    currentModelId: string | null | undefined,
    catalog: CatalogEntry[],
    assessment: GpuAssessment | null | undefined,
): CatalogEntry | null {
    if (!currentModelId || assessment?.state !== 'ready') return null;
    const current = catalog.find((m) => m.name.toLowerCase() === currentModelId.toLowerCase());
    if (!current || current.runtime.deviceType !== 'CPU') return null;
    return (
        catalog.find(
            (m) => m.alias.toLowerCase() === current.alias.toLowerCase() && m.runtime.deviceType === 'GPU' && !m.incompatible,
        ) ?? null
    );
}

export interface ModelsTabNotice {
    kind: 'catalog-error' | 'gpu';
    tone: CardTone;
    headline: string;
    lines: string[];
}

/**
 * What the Models tab says above the list. A catalog that failed to read comes first and is never
 * worded as a GPU problem (the empty list it leaves behind used to be shown as "No GPU models are
 * available"). With the GPU filter and no GPU models, the headline is the backend's stated cause.
 */
export function modelsTabNotice(args: {
    loading: boolean;
    catalogError: string | null;
    gpuFilterEmpty: boolean;
    assessment: GpuAssessment | null;
}): ModelsTabNotice | null {
    if (args.loading) return null;
    // The backend also says so itself (a failed read arrives as an empty list on some paths).
    const backendSaysCatalogFailed = args.assessment?.state === 'catalog_failed';
    if (args.catalogError || backendSaysCatalogFailed) {
        return {
            kind: 'catalog-error',
            tone: 'red',
            headline: 'The model list could not be read.',
            lines: [
                ...(args.catalogError ? [args.catalogError] : (args.assessment?.detail ?? []).filter((d) => d.toLowerCase().includes('model list'))),
                'This is not a GPU problem. Choose Refresh, or restart Plugable Chat.',
            ],
        };
    }
    if (!args.gpuFilterEmpty) return null;
    const a = args.assessment;
    if (!a || a.state === 'checking') {
        return { kind: 'gpu', tone: 'info', headline: 'Checking GPU acceleration...', lines: [] };
    }
    // An assessment of "ready" with an empty GPU list means the list was read before the engine
    // finished: say that instead of claiming the GPU is missing.
    if (a.state === 'ready') {
        return { kind: 'gpu', tone: 'info', headline: 'GPU acceleration is ready; the model list is refreshing.', lines: [] };
    }
    return { kind: 'gpu', tone: cardTone(a), headline: a.headline, lines: cardLines(a).slice(1) };
}

/** `qwen3.5-4b-cuda-gpu:4` runs on the GPU; ids without a device marker are CPU builds. Mirrors Device::from_model_id. */
export function deviceFromModelId(id: string | null | undefined): 'GPU' | 'CPU' | null {
    if (!id || id === 'Loading...' || id === 'No models' || id === 'Downloading...') return null;
    const base = id.toLowerCase().split(':')[0];
    return /-(gpu|cuda|trt|webgpu|dml)\b|-gpu/.test(base) ? 'GPU' : 'CPU';
}
