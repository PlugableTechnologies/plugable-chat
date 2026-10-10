import { useState, useEffect } from 'react';
import { Cpu, RefreshCw, AlertCircle, X, Loader2, HardDrive, ExternalLink } from 'lucide-react';
import { invoke, type FoundryCatalogModel, type FoundryServiceStatus } from '../../../lib/api';
import { useChatStore } from '../../../store/chat-store';
import { FoundryModelCard } from '../cards/FoundryModelCard';
import type { ModelDeviceFilter } from '../types';
import { describeGpuDiagnostics, effectiveDeviceFilter, type GpuDiagnostics } from '../../../store/chat/gpu-diagnostics';

export function ModelsTab() {
    const [catalogModels, setCatalogModels] = useState<FoundryCatalogModel[]>([]);
    const [cachedModelIds, setCachedModelIds] = useState<string[]>([]);
    const [loadedModelIds, setLoadedModelIds] = useState<string[]>([]);
    const [serviceStatus, setServiceStatus] = useState<FoundryServiceStatus | null>(null);
    const [deviceFilter, setDeviceFilter] = useState<ModelDeviceFilter>('GPU');
    const [isLoading, setIsLoading] = useState(true);
    const [downloadingModel, setDownloadingModel] = useState<string | null>(null);
    const [downloadProgress, setDownloadProgress] = useState<{ file: string; progress: number } | null>(null);
    const [error, setError] = useState<string | null>(null);

    const [gpuDiagnostics, setGpuDiagnostics] = useState<GpuDiagnostics | null>(null);
    const [retryingGpu, setRetryingGpu] = useState(false);

    const { operations, dispatchOperation } = useChatStore();
    const modelDownloadOp = operations['model-download'];

    // Fetch all data on mount
    useEffect(() => {
        fetchAllData();
    }, []);

    // Track download progress from the keyed model-download operation
    useEffect(() => {
        if (modelDownloadOp?.state === 'active') {
            setDownloadProgress({
                file: modelDownloadOp.file || '',
                progress: modelDownloadOp.percent || 0,
            });
        }
    }, [modelDownloadOp]);

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
        } catch (err) {
            console.error('Failed to fetch catalog models:', err);
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

    const handleDownload = async (model: FoundryCatalogModel) => {
        setDownloadingModel(model.name);
        setDownloadProgress({ file: 'Starting...', progress: 0 });
        dispatchOperation({ type: 'progress', key: 'model-download', message: `Downloading ${model.alias || model.name}`, percent: 0 });
        try {
            await invoke('download_model', { modelName: model.name });
            await fetchCachedModels();
            dispatchOperation({ type: 'done', key: 'model-download', message: `${model.alias || model.name} downloaded successfully` });
        } catch (err) {
            console.error('Download failed:', err);
            const errorMessage = `Failed to download ${model.alias || model.name}:\n\n${err}`;
            alert(errorMessage);
            setError(`Download failed: ${err}`);
            dispatchOperation({ type: 'error', key: 'model-download', message: `Failed to download ${model.alias || model.name}: ${err}` });
        } finally {
            setDownloadingModel(null);
            setDownloadProgress(null);
        }
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

    // Ask the backend why GPU acceleration is unavailable when the GPU list is empty.
    useEffect(() => {
        if (isLoading || !gpuEmpty) return;
        invoke<GpuDiagnostics>('get_gpu_diagnostics')
            .then(setGpuDiagnostics)
            .catch((e) => console.warn('get_gpu_diagnostics failed', e));
    }, [isLoading, gpuEmpty]);

    const handleRetryGpu = async () => {
        setRetryingGpu(true);
        try {
            await invoke('retry_gpu_registration');
            await fetchAllData();
            setGpuDiagnostics(await invoke<GpuDiagnostics>('get_gpu_diagnostics'));
        } catch (e) {
            setError(`GPU retry failed: ${e}`);
        } finally {
            setRetryingGpu(false);
        }
    };

    const filteredModels = catalogModels
        .filter((model) => {
            if (activeFilter === 'Auto') return true;
            return model.runtime?.deviceType === activeFilter;
        })
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

                {/* GPU unavailable: explain why, offer retry */}
                {!isLoading && gpuEmpty && (
                    <div className="gpu-diagnostics mb-4 p-3 bg-amber-50 border border-amber-200 rounded-lg text-sm text-amber-900" role="alert">
                        <p className="font-medium">No GPU models are available.</p>
                        {describeGpuDiagnostics(gpuDiagnostics).map((line) => (
                            <p key={line} className="mt-1">{line}</p>
                        ))}
                        {cpuFallback && (
                            <p className="mt-1">Showing CPU models instead. They run on any computer but more slowly.</p>
                        )}
                        <button
                            onClick={handleRetryGpu}
                            disabled={retryingGpu}
                            className="gpu-retry-button mt-2 px-3 py-1 rounded-md bg-amber-600 text-white hover:bg-amber-700 disabled:opacity-50"
                        >
                            {retryingGpu ? 'Retrying…' : 'Retry GPU setup'}
                        </button>
                    </div>
                )}

                {/* Model Cards Grid */}
                {!isLoading && (
                    <div className="model-card-grid grid grid-cols-1 gap-4">
                        {filteredModels.map((model) => (
                            <FoundryModelCard
                                key={model.name}
                                model={model}
                                isCached={isModelCached(model)}
                                isLoaded={isModelLoaded(model)}
                                isDownloading={downloadingModel === model.name}
                                downloadProgress={downloadingModel === model.name ? downloadProgress ?? undefined : undefined}
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
                        <p>No models found for {activeFilter} device type.</p>
                        <p className="text-sm mt-2">Try selecting a different device filter.</p>
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
