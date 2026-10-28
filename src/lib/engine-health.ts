// What Settings > Models says about the engine's own health, kept pure so it can be tested.
// The backend (engine_health.rs) decides; this only words it.

/** How the previous run died (crash_capture.rs); `inEp` means the faulting module was a downloaded GPU provider. */
export interface LastCrash {
  diedIn: string;
  inEp: boolean;
  provider: string | null;
  faultingModule: string | null;
  dumpPath: string | null;
}

export interface EngineHealth {
  gpuOff: boolean;
  gpuOffReason: string | null;
  uncleanDeaths: number;
  lastQuarantine: string | null;
  lastCrash?: LastCrash | null;
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
    const where = h.lastCrash?.inEp ? ` The crash was inside the GPU component${h.lastCrash.provider ? ` "${h.lastCrash.provider}"` : ''}.` : '';
    const dump = h.lastCrash?.dumpPath ? ` A crash dump was saved at ${h.lastCrash.dumpPath}.` : '';
    return {
      tone: 'info',
      headline: 'GPU components were set aside after a crash',
      lines: [`They are downloaded again automatically (a CPU model is used until then if you are offline).${where}${dump}`],
      action: null,
    };
  }
  return null;
}
