// What Settings > Models says about the engine's own health, kept pure so it can be tested.
// The backend (engine_health.rs) decides; this only words it.

export interface EngineHealth {
  gpuOff: boolean;
  gpuOffReason: string | null;
  uncleanDeaths: number;
  lastQuarantine: string | null;
}

export interface EngineNotice {
  tone: 'amber' | 'info';
  headline: string;
  lines: string[];
  /** The one button that fits: turn GPU back on, or nothing special. */
  action: 'try-gpu-again' | null;
}

/** A banner only when the engine did something on its own; null when all is well. */
export function engineNotice(h: EngineHealth | null): EngineNotice | null {
  if (!h) return null;
  if (h.gpuOff) {
    return {
      tone: 'amber',
      headline: 'GPU acceleration is switched off',
      lines: [
        h.gpuOffReason ?? 'Plugable Chat closed unexpectedly more than once in a row.',
        'A CPU model is used so you can keep working. Try the GPU again when you are ready; if it closes again it will switch off again.',
      ],
      action: 'try-gpu-again',
    };
  }
  if (h.lastQuarantine) {
    return {
      tone: 'info',
      headline: 'GPU components were set aside after a crash',
      lines: ['They are downloaded again automatically (a CPU model is used until then if you are offline).'],
      action: null,
    };
  }
  return null;
}
