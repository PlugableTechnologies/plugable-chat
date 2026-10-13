import { useState } from 'react';
import { invoke } from '../lib/api';
import { copySupportDetails } from '../lib/support';

/** "Copy details for support" for any card that shows a problem. `shown` is what the card says. */
export function SupportCopyButton({ shown, className = '' }: { shown: string; className?: string }) {
    const [state, setState] = useState<'idle' | 'busy' | 'copied' | 'failed'>('idle');
    const copy = async () => {
        setState('busy');
        const r = await copySupportDetails(shown, {
            invoke,
            writeClipboard: (t) => navigator.clipboard.writeText(t),
        });
        setState(r.ok ? 'copied' : 'failed');
        setTimeout(() => setState('idle'), 3000);
    };
    return (
        <button
            onClick={copy}
            disabled={state === 'busy'}
            className={`support-copy-button flex-shrink-0 px-3 py-1 rounded-md text-sm font-medium border border-current hover:bg-black/5 disabled:opacity-50 ${className}`}
            title="Copies version, machine, GPU and log details to paste into an email"
        >
            {state === 'busy' ? 'Collecting…' : state === 'copied' ? 'Copied' : state === 'failed' ? 'Copy failed' : 'Copy details for support'}
        </button>
    );
}
