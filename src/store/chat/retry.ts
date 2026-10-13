import type { OperationAction, OperationKey } from './operations.ts';
import type { GpuAssessment } from '../../lib/gpu-assessment.ts';
import { cardTone } from '../../lib/gpu-assessment.ts';

export interface RetryDeps {
    invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
    dispatch: (action: OperationAction) => void;
    refreshModels: () => Promise<void>;
}

const attempts: Partial<Record<OperationKey, number>> = {};

/** Forget attempt counts (tests, and after a success). */
export function resetRetryAttempts(): void {
    for (const k of Object.keys(attempts)) delete attempts[k as OperationKey];
}

/**
 * Run a Retry and leave the row in a state that says what happened. Every outcome is explicit:
 * a Retry that finds the work already done clears its row (it used to stay on "Retrying..." for
 * ever), and a Retry that fails again says it is attempt N with the new reason, so it never looks
 * like the button did nothing.
 */
export async function runRetry(
    key: OperationKey,
    spec: { command: string; args?: Record<string, unknown> },
    deps: RetryDeps,
): Promise<void> {
    const n = (attempts[key] ?? 0) + 1;
    attempts[key] = n;
    deps.dispatch({ type: 'progress', key, message: n > 1 ? `Retrying (attempt ${n})...` : 'Retrying...', percent: 0 });
    try {
        const result = await deps.invoke<unknown>(spec.command, spec.args);
        switch (key) {
            case 'ep-registration': {
                const a = result as GpuAssessment | null;
                // `error` only when the GPU is still unusable AND the cause is a fault, not "no GPU".
                if (a && cardTone(a) !== 'none' && cardTone(a) !== 'info') {
                    deps.dispatch({ type: 'error', key, message: `${a.headline} (attempt ${n})` });
                } else {
                    delete attempts[key];
                    deps.dispatch({ type: 'clear', key });
                }
                await deps.refreshModels();
                break;
            }
            case 'embedding':
                delete attempts[key];
                deps.dispatch({ type: 'done', key, message: 'Search model ready' });
                break;
            case 'model-download':
            case 'toolbox-download':
                delete attempts[key];
                deps.dispatch({ type: 'done', key });
                if (key === 'model-download') await deps.refreshModels();
                break;
        }
    } catch (e) {
        const reason = e instanceof Error ? e.message : typeof e === 'string' ? e : JSON.stringify(e);
        deps.dispatch({ type: 'error', key, message: `Retry ${n} failed: ${reason}` });
    }
}
