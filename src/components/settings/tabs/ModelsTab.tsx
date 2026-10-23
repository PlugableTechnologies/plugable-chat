import { useState, useEffect } from 'react';
import { Cpu, Search, RefreshCw, AlertCircle, X, Loader2, HardDrive, ExternalLink } from 'lucide-react';
import { invoke, type FoundryCatalogModel, type FoundryServiceStatus } from '../../../lib/api';
import { useChatStore } from '../../../store/chat-store';
import { FoundryModelCard } from '../cards/FoundryModelCard';
import type { ModelDeviceFilter } from '../types';
import { effectiveDeviceFilter } from '../../../store/chat/gpu-diagnostics';
import { modelsTabNotice, type GpuAssessment } from '../../../lib/gpu-assessment';
import { SupportCopyButton } from '../../SupportCopyButton';
import { matchesModelQuery } from '../../../lib/model-filter';
import { isDownloadPending, type DownloadEntry } from '../../../store/chat/downloads';
import { engineNotice, type EngineHealth } from '../../../lib/engine-health';

function downloadProgressFor(d: DownloadEntry | undefined) {
    if (!isDownloadPending(d)) return undefined;
    return { file: d!.state === 'queued' ? 'Queued…' : d!.label, progress: d!.percent };
}

export function ModelsTab() {
    const [catalogModels, setCatalogModels] = useState<FoundryCatalogModel[]>([]);
    const [cachedModelIds, setCachedModelIds] = useState<string[]>([]);
    const [loadedModelIds, setLoadedModelIds] = useState<string[]>([]);
    const [serviceStatus, setServiceStatus] = useState<FoundryServiceStatus | null>(null);
    const [deviceFilter, setDeviceFilter] = useState<ModelDeviceFilter>('GPU');
    const [isLoading, setIsLoading] = useState(true);
    const [searchQuery, setSearchQuery] = useState('');
    const [error, setError] = useState<string | null>(null);

    const [assessment, setAssessment] = useState<GpuAssessment | null>(null);
    const [catalogError, setCatalogError] = useState<string | null>(null);
    const [retryingGpu, setRetryingGpu] = useState(false);
    const [engineHealth, setEngineHealth] = useState<EngineHealth | null>(null);
    const [repairing, setRepairing] = useState(false);
    const [repairDone, setRepairDone] = useState(false);

    const { downloads, downloadModel, cancelModelDownload } = useChatStore();

    // Fetch all data on mount
    useEffect(() => {
        fetchAllData();
    }, []);

    // A download finished somewhere (here, the status bar retry, or a fallback): refresh "Downloaded".
    const doneCount = Object.values(downloads).filter((d) => d.state === 'done').length;
    useEffect(() => {
        if (doneCount > 0) fetchCachedModels();
    }, [doneCount]);

    const fetchAllData = async () => {
        setIsLoading(true);
        setError(null);
        try {
            await Promise.all([
                fetchCatalogModels(),
                fetchCachedModels(),
                fetchLoadedModels(),
                fetchServiceStatus(),
            ]);
        } catch (err) {
            setError(err instanceof Error ? err.message : 'Failed to load model data');
        } finally {
            setIsLoading(false);
        }
    };

    const fetchCatalogModels = async () => {
        try {
            const models = await invoke<FoundryCatalogModel[]>('get_catalog_models');
            setCatalogModels(models);
            setCatalogError(null);
        } catch (err) {
            console.error('Failed to fetch catalog models:', err);
            // Keep the reason: an empty list from a failed read must not be shown as "no GPU models".
            setCatalogError(err instanceof Error ? err.message : String(err));
        }
    };

    const fetchCachedModels = async () => {
        try {
            const models = await invoke<string[]>('get_models');
            setCachedModelIds(models);
        } catch (err) {
            console.error('Failed to fetch cached models:', err);
        }
    };

    const fetchLoadedModels = async () => {
        try {
            const models = await invoke<string[]>('get_loaded_models');
            setLoadedModelIds(models);
        } catch (err) {
            console.error('Failed to fetch loaded models:', err);
        }
    };

    const fetchServiceStatus = async () => {
        try {
            const status = await invoke<FoundryServiceStatus>('get_foundry_service_status');
            setServiceStatus(status);
        } catch (err) {
            console.error('Failed to fetch service status:', err);
        }
    };

    // The store tracks every download (progress, queueing, cancel, completion), so a second
    // download no longer replaces the first and nothing is left at 100%.
    const handleDownload = async (model: FoundryCatalogModel) => {
        const ok = await downloadModel(model.name, model.alias || model.name);
        if (ok) await fetchCachedModels();
    };

    const handleUnload = async (model: FoundryCatalogModel) => {
        try {
            await invoke('unload_model', { modelName: model.name });
            await fetchLoadedModels();
        } catch (err) {
            console.error('Unload failed:', err);
            const errorMessage = `Failed to unload ${model.alias || model.name}:\n\n${err}`;
            alert(errorMessage);
            setError(`Unload failed: ${err}`);
        }
    };

    const handleRemove = async (model: FoundryCatalogModel) => {
        if (!confirm(`Remove ${model.alias || model.name} from cache?\n\nThis will delete the downloaded model files from disk.`)) {
            return;
        }
        try {
            await invoke('remove_cached_model', { modelName: model.name });
            // Refresh the cached models list to reflect the removal
            await fetchCachedModels();
            // Also refresh loaded models in case it was loaded
            await fetchLoadedModels();
        } catch (err) {
            console.error('Remove failed:', err);
            const errorMessage = `Failed to remove ${model.alias || model.name} from cache:\n\n${err}`;
            alert(errorMessage);
            setError(`Remove failed: ${err}`);
        }
    };

    const handleOpenProductLink = () => {
        window.open('https://plugable.com/products/tbt5-ai', '_blank');
    };

    // Filter and sort models. Models with a compatibility warning are still listed and
    // installable (the card shows a "May not run here" badge) — we don't hide them.
    const deviceCounts: Record<string, number> = {};
    for (const m of catalogModels) {
        const d = m.runtime?.deviceType ?? 'unknown';
        deviceCounts[d] = (deviceCounts[d] ?? 0) + 1;
    }
    const { filter: activeFilter, fellBack: cpuFallback } = effectiveDeviceFilter(deviceFilter, deviceCounts);
    const gpuEmpty = deviceFilter === 'GPU' && !(deviceCounts.GPU ?? 0);

    // Ask the backend why the GPU list is empty; it asks Windows again rather than reusing an old answer.
    useEffect(() => {
        if (isLoading || !gpuEmpty) return;
        invoke<GpuAssessment>('get_gpu_assessment', { refresh: true })
            .then(setAssessment)
            .catch((e) => console.warn('get_gpu_assessment failed', e));
    }, [isLoading, gpuEmpty]);

    // The backend waits for the whole attempt (and re-reads its model list) before answering, so
    // the list refreshed here is current.
    useEffect(() => {
        invoke<EngineHealth>('get_engine_health').then(setEngineHealth).catch(() => setEngineHealth(null));
    }, []);

    // No reinstall or reboot: set the GPU components aside (they download again), forget crash
    // history and the GPU-off switch, then run GPU setup.
    const handleRepair = async () => {
        setRepairing(true);
        setRepairDone(false);
        try {
            await invoke('repair_engine');
            setRepairDone(true);
            setEngineHealth(await invoke<EngineHealth>('get_engine_health'));
        } catch (e) {
            setError(`Repair failed: ${e instanceof Error ? e.message : e}`);
        } finally {
            setRepairing(false);
        }
    };

    const handleTryGpuAgain = async () => {
        setRepairing(true);
        try {
            await invoke('try_gpu_again');
            setEngineHealth(await invoke<EngineHealth>('get_engine_health'));
        } catch (e) {
            setError(`Could not turn the GPU back on: ${e instanceof Error ? e.message : e}`);
        } finally {
            setRepairing(false);
        }
    };

    const handleRetryGpu = async () => {
        setRetryingGpu(true);
        try {
            setAssessment(await invoke<GpuAssessment>('retry_gpu_registration'));
            await fetchAllData();
        } catch (e) {
            setError(`GPU retry failed: ${e instanceof Error ? e.message : e}`);
        } finally {
            setRetryingGpu(false);
        }
    };

    const notice = modelsTabNotice({ loading: isLoading, catalogError, gpuFilterEmpty: gpuEmpty, assessment });

    const filteredModels = catalogModels
        .filter((model) => {
            if (activeFilter === 'Auto') return true;
            return model.runtime?.deviceType === activeFilter;
        })
        .filter((model) => matchesModelQuery(model, searchQuery))
        .sort((a, b) => {
            // Sort: 1. Tools support first, 2. By size ascending (smaller first)
            const aTools = a.supportsToolCalling ? 1 : 0;
            const bTools = b.supportsToolCalling ? 1 : 0;
            if (aTools !== bTools) return bTools - aTools; // Tools support first

            // Then by size ascending (smaller models first)
            return (a.fileSizeMb || 0) - (b.fileSizeMb || 0);
        });

    const isModelCached = (model: FoundryCatalogModel) => {
        return cachedModelIds.some(id => 
            id.toLowerCase().includes(model.name.toLowerCase()) || 
            model.name.toLowerCase().includes(id.toLowerCase())
        );
    };

    const isModelLoaded = (model: FoundryCatalogModel) => {
        return loadedModelIds.some(id => 
            id.toLowerCase().includes(model.name.toLowerCase()) || 
            model.name.toLowerCase().includes(id.toLowerCase())
        );
    };

    const serviceEndpoint = serviceStatus?.endpoints?.[0] || 'Not available';

    return (
        <div className="models-tab flex flex-col h-full">
            {/* Header */}
            <div className="flex items-center justify-between mb-4">
                <div className="flex items-center gap-4">
                    {/* Device Filter */}
                    <div className="flex items-center gap-2">
                        <Cpu size={16} className="text-gray-500" />
                        <select
                            value={deviceFilter}
                            onChange={(e) => setDeviceFilter(e.target.value as ModelDeviceFilter)}
                            className="text-sm border border-gray-200 rounded-lg px-3 py-1.5 bg-white focus:outline-none focus:ring-2 focus:ring-blue-400"
                        >
                            <option value="Auto">All Devices</option>
                            <option value="GPU">GPU</option>
                            <option value="CPU">CPU</option>
                            <option value="NPU">NPU</option>
                        </select>
                    </div>

                    {/* Search */}
                    <div className="flex items-center gap-2">
                        <Search size={16} className="text-gray-500" />
                        <input
                            type="search"
                            value={searchQuery}
                            onChange={(e) => setSearchQuery(e.target.value)}
                            placeholder="Search models"
                            aria-label="Search models"
                            className="model-search text-sm border border-gray-200 rounded-lg px-3 py-1.5 bg-white w-44 focus:outline-none focus:ring-2 focus:ring-blue-400"
                        />
                    </div>

                    {/* Service Status */}
                    <div className="flex items-center gap-2 text-sm text-gray-500">
                        <span className={`w-2 h-2 rounded-full ${serviceStatus ? 'bg-green-500' : 'bg-red-500'}`} />
                        <span>{serviceStatus ? `Service: ${serviceEndpoint}` : 'Service not available'}</span>
                    </div>
                </div>

                {/* Refresh Button */}
                <button
                    onClick={fetchAllData}
                    disabled={isLoading}
                    className="flex items-center gap-2 px-3 py-1.5 text-sm text-gray-600 bg-gray-100 rounded-lg hover:bg-gray-200 transition-colors disabled:opacity-50"
                >
                    <RefreshCw size={14} className={isLoading ? 'animate-spin' : ''} />
                    Refresh
                </button>
            </div>

            {/* Error display */}
            {error && (
                <div className="mb-4 p-3 bg-red-50 border border-red-200 rounded-lg text-red-700 text-sm flex items-center gap-2">
                    <AlertCircle size={16} />
                    {error}
                    <button onClick={() => setError(null)} className="ml-auto text-red-500 hover:text-red-700">
                        <X size={14} />
                    </button>
                </div>
            )}

            {/* Scrollable content area */}
            <div className="flex-1 overflow-y-auto">
                {/* Promotional Banner */}
                <div className="mb-4 p-3 bg-gradient-to-r from-blue-50 to-indigo-50 border border-blue-100 rounded-lg">
                    <p className="text-sm text-gray-700">
                        Enable more models with the{' '}
                        <button
                            onClick={handleOpenProductLink}
                            className="text-blue-600 hover:text-blue-800 font-medium inline-flex items-center gap-1 hover:underline"
                        >
                            Plugable TBT5-AI
                            <ExternalLink size={12} />
                        </button>
                    </p>
                </div>

                {/* Loading state */}
                {isLoading && (
                    <div className="flex items-center justify-center py-12">
                        <Loader2 size={24} className="animate-spin text-blue-500" />
                    </div>
                )}

                {/* Why the list is empty: a failed list read, or the stated GPU cause */}
                {notice && (
                    <div
                        className={`gpu-diagnostics mb-4 p-3 border rounded-lg text-sm ${notice.tone === 'red' ? 'bg-red-50 border-red-300 text-red-900' : notice.tone === 'info' ? 'bg-blue-50 border-blue-200 text-blue-900' : 'bg-amber-50 border-amber-200 text-amber-900'}`}
                        role="alert"
                    >
                        <p className="font-medium">{notice.headline}</p>
                        {notice.lines.map((line) => (
                            <p key={line} className="mt-1">{line}</p>
                        ))}
                        {notice.kind === 'gpu' && cpuFallback && (
                            <p className="mt-1">Showing CPU models instead. They run on any computer but more slowly.</p>
                        )}
                        <div className="mt-2 flex items-center gap-2">
                            {(notice.kind === 'catalog-error' || assessment?.retryMayHelp !== false) && (
                                <button
                                    onClick={notice.kind === 'catalog-error' ? fetchAllData : handleRetryGpu}
                                    disabled={retryingGpu}
                                    className="gpu-retry-button px-3 py-1 rounded-md bg-amber-600 text-white hover:bg-amber-700 disabled:opacity-50"
                                >
                                    {retryingGpu ? 'Retrying…' : notice.kind === 'catalog-error' ? 'Refresh the list' : 'Retry GPU setup'}
                                </button>
                            )}
                            <SupportCopyButton shown={[notice.headline, ...notice.lines].join('\n')} />
                        </div>
                    </div>
                )}

                {/* The engine acted on its own after a crash, or the user wants a clean start */}
                {(() => {
                    const en = engineNotice(engineHealth);
                    return (
                        <div
                            className={`engine-health mb-4 p-3 border rounded-lg text-sm ${en?.tone === 'amber' ? 'bg-amber-50 border-amber-200 text-amber-900' : 'bg-gray-50 border-gray-200 text-gray-700'}`}
                        >
                            {en && (
                                <>
                                    <p className="font-medium">{en.headline}</p>
                                    {en.lines.map((line) => (
                                        <p key={line} className="mt-1">{line}</p>
                                    ))}
                                </>
                            )}
                            <div className="mt-2 flex items-center gap-2">
                                {en?.action === 'try-gpu-again' && (
                                    <button
                                        onClick={handleTryGpuAgain}
                                        disabled={repairing}
                                        className="engine-try-gpu px-3 py-1 rounded-md bg-amber-600 text-white hover:bg-amber-700 disabled:opacity-50"
                                    >
                                        Try GPU again
                                    </button>
                                )}
                                <button
                                    onClick={handleRepair}
                                    disabled={repairing}
                                    title="Sets the downloaded GPU components aside so they download again, and clears crash history. Your models and chats are not touched."
                                    className="engine-repair px-3 py-1 rounded-md bg-white border border-gray-300 hover:bg-gray-50 disabled:opacity-50"
                                >
                                    {repairing ? 'Repairing…' : 'Repair engine'}
                                </button>
                                {repairDone && <span className="text-green-700">Repaired. GPU components are downloading again.</span>}
                            </div>
                        </div>
                    );
                })()}

                {/* Model Cards Grid */}
                {!isLoading && (
                    <div className="model-card-grid grid grid-cols-1 gap-4">
                        {filteredModels.map((model) => (
                            <FoundryModelCard
                                key={model.name}
                                model={model}
                                isCached={isModelCached(model)}
                                isLoaded={isModelLoaded(model)}
                                isDownloading={isDownloadPending(downloads[model.name])}
                                downloadProgress={downloadProgressFor(downloads[model.name])}
                                onCancelDownload={() => cancelModelDownload(model.name)}
                                onClearWarning={async () => {
                                    await invoke('clear_model_incompatible', { modelName: model.name });
                                    await fetchAllData();
                                }}
                                onDownload={() => handleDownload(model)}
                                onUnload={() => handleUnload(model)}
                                onRemove={() => handleRemove(model)}
                            />
                        ))}
                    </div>
                )}

                {/* Empty state */}
                {!isLoading && filteredModels.length === 0 && (
                    <div className="text-center py-12 text-gray-500">
                        <Cpu size={48} className="mx-auto mb-4 text-gray-300" />
                        <p>{searchQuery.trim() ? `No models match "${searchQuery.trim()}".` : `No models found for ${activeFilter} device type.`}</p>
                        <p className="text-sm mt-2">{searchQuery.trim() ? 'Clear the search or try a different device filter.' : 'Try selecting a different device filter.'}</p>
                    </div>
                )}
            </div>

            {/* Footer */}
            <div className="mt-4 pt-4 border-t border-gray-100 text-xs text-gray-400">
                <div className="flex items-center gap-2">
                    <HardDrive size={12} />
                    <span>Cache: {serviceStatus?.modelDirPath || 'Unknown location'}</span>
                </div>
            </div>
        </div>
    );
}
