/** Result of the backend `get_gpu_diagnostics` command. */
export interface GpuDiagnostics {
    registered: string[];
    failed: { ep: string; kind: GpuFailureKind; message: string }[];
    driver_version?: string;
    vcpp_installed?: boolean;
}

export type GpuFailureKind = 'network' | 'driver' | 'missing_dll' | 'cancelled' | 'unsupported' | 'unknown';

const KIND_TEXT: Record<GpuFailureKind, string> = {
    network: 'The GPU components could not be downloaded. Check the internet connection (or proxy) and retry.',
    driver: 'The graphics driver is too old or missing. Update the NVIDIA driver, then retry.',
    missing_dll: 'A required system library is missing (usually the Microsoft Visual C++ runtime). Install it, then retry.',
    cancelled: 'GPU setup was cancelled. Retry to download the GPU components.',
    unsupported: 'This GPU is not supported by the available acceleration providers.',
    unknown: 'GPU setup failed for an unrecognised reason.',
};

/** One human-readable explanation per failure, plus preflight facts that explain it. */
export function describeGpuDiagnostics(d: GpuDiagnostics | null): string[] {
    if (!d) return [];
    const lines = d.failed.map((f) => `${f.ep}: ${KIND_TEXT[f.kind] ?? KIND_TEXT.unknown} (${f.message})`);
    if (d.vcpp_installed === false) lines.push('Microsoft Visual C++ runtime was not found on this computer.');
    if (d.driver_version) lines.push(`Detected graphics driver version: ${d.driver_version}`);
    if (d.failed.length === 0 && d.registered.length === 0) lines.push('No GPU acceleration providers were registered.');
    return lines;
}

/** Device filter to show when the preferred one has no models: CPU if the GPU list is empty and CPU is not. */
export function effectiveDeviceFilter<T extends string>(
    preferred: T,
    countsByDevice: Partial<Record<string, number>>,
): { filter: T | 'CPU'; fellBack: boolean } {
    if (preferred === 'GPU' && !(countsByDevice.GPU ?? 0) && (countsByDevice.CPU ?? 0) > 0) {
        return { filter: 'CPU', fellBack: true };
    }
    return { filter: preferred, fellBack: false };
}
