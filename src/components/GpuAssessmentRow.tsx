import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { invoke, type FoundryCatalogModel } from '../lib/api';
import { useChatStore } from '../store/chat-store';
import { cardLines, cardText, cardTone, findGpuUpgrade } from '../lib/gpu-assessment';
import { SupportCopyButton } from './SupportCopyButton';

/**
 * Two cards under the status bar: why the GPU is not in use (named causes, both views of the
 * machine, Retry and Copy details), and, once a GPU becomes usable while a CPU build is loaded,
 * an offer to switch.
 */
export function GpuAssessmentRow() {
    const { gpuAssessment, gpuAssessmentDismissed, dismissGpuAssessment, retryOperation, currentModel, downloadModel, loadModel } = useChatStore();
    const [catalog, setCatalog] = useState<FoundryCatalogModel[]>([]);
    const [switching, setSwitching] = useState(false);

    // Only needed to find a GPU build of the loaded model once the GPU is usable.
    useEffect(() => {
        if (gpuAssessment?.state !== 'ready') return;
        invoke<FoundryCatalogModel[]>('get_catalog_models').then(setCatalog).catch((e) => console.warn('get_catalog_models failed', e));
    }, [gpuAssessment?.state]);

    const upgrade = findGpuUpgrade(currentModel, catalog, gpuAssessment);
    const tone = cardTone(gpuAssessment);

    return (
        <>
            {gpuAssessment && (tone === 'amber' || tone === 'red') && gpuAssessmentDismissed !== gpuAssessment.headline && (
                <div
                    className={`gpu-assessment-row flex items-start justify-between px-4 py-2 border-b ${tone === 'red' ? 'bg-red-50 border-red-300 text-red-800' : 'bg-amber-50 border-amber-200 text-amber-900'}`}
                    role="alert"
                >
                    <div className="flex-1 min-w-0 text-sm">
                        <div className="font-medium">{gpuAssessment.headline}</div>
                        {cardLines(gpuAssessment).slice(1).map((line) => (
                            <div key={line} className="mt-0.5 opacity-90">{line}</div>
                        ))}
                    </div>
                    <div className="flex items-center gap-2 ml-3 flex-shrink-0">
                        {gpuAssessment.retryMayHelp && (
                            <button
                                onClick={() => retryOperation('ep-registration')}
                                className="gpu-assessment-retry px-3 py-1 rounded-md text-sm font-medium bg-amber-600 text-white hover:bg-amber-700"
                            >
                                Retry GPU setup
                            </button>
                        )}
                        <SupportCopyButton shown={cardText(gpuAssessment)} />
                        <button onClick={dismissGpuAssessment} className="p-1 rounded-full hover:bg-black/5" aria-label="Dismiss GPU notice">
                            <X size={16} />
                        </button>
                    </div>
                </div>
            )}
            {upgrade && (
                <div className="gpu-upgrade-row flex items-center justify-between px-4 py-2 bg-green-50 border-b border-green-200 text-green-800" role="status">
                    <span className="text-sm font-medium">
                        The GPU is ready. Switch to the faster GPU version of this model?
                    </span>
                    <button
                        disabled={switching}
                        onClick={async () => {
                            setSwitching(true);
                            try {
                                await downloadModel(upgrade.name);
                                await loadModel(upgrade.name);
                            } finally {
                                setSwitching(false);
                            }
                        }}
                        className="gpu-upgrade-button flex-shrink-0 ml-3 px-3 py-1 rounded-md text-sm font-medium bg-green-600 text-white hover:bg-green-700 disabled:opacity-50"
                    >
                        {switching ? 'Switching…' : 'Switch to GPU'}
                    </button>
                </div>
            )}
        </>
    );
}
