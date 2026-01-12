import { HardDrive, Wrench, CheckCircle, Zap, Download, Trash2, Loader2 } from 'lucide-react';
import type { FoundryModelCardProps } from '../types';

export function FoundryModelCard({
    model,
    isCached,
    isLoaded,
    isDownloading,
    downloadProgress,
    onDownload,
    onUnload,
    onRemove,
}: FoundryModelCardProps) {
    const deviceType = model.runtime?.deviceType || 'CPU';
    const fileSizeGB = (model.fileSizeMb / 1024).toFixed(2);
    const tasks = model.task?.replace('-completion', '') || 'chat';
    const supportsTools = model.supportsToolCalling;

    // Device badge colors
    const deviceBadgeClass = deviceType === 'GPU'
        ? 'bg-emerald-100 text-emerald-700'
        : deviceType === 'NPU'
            ? 'bg-purple-100 text-purple-700'
            : 'bg-gray-100 text-gray-600';

    // Card border for loaded state
    const cardBorderClass = isLoaded
        ? 'border-2 border-blue-400 shadow-lg shadow-blue-100'
        : 'border border-gray-200';

    return (
        <div className={`model-card bg-white rounded-xl p-4 ${cardBorderClass} transition-all duration-200 hover:shadow-md`}>
            {/* Header: Device badge, alias, license */}
            <div className="flex items-center justify-between mb-2">
                <div className="flex items-center gap-2">
                    <span className={`text-xs font-semibold px-2 py-0.5 rounded ${deviceBadgeClass}`}>
                        {deviceType}
                    </span>
                    <h3 className="font-semibold text-gray-900">{model.alias || model.displayName}</h3>
                </div>
                <span className="text-xs text-gray-500 bg-gray-50 px-2 py-0.5 rounded">
                    {model.license || 'Unknown'}
                </span>
            </div>

            {/* Model ID subtitle */}
            <p className="text-xs text-gray-400 font-mono mb-3 truncate" title={model.name}>
                {model.name}
            </p>

            {/* Details row */}
            <div className="flex items-center gap-3 text-sm text-gray-600 mb-3">
                <div className="flex items-center gap-1">
                    <HardDrive size={14} className="text-gray-400" />
                    <span>{fileSizeGB} GB</span>
                </div>
                <span className="text-gray-300">|</span>
                <span className="capitalize">{tasks}</span>
                {supportsTools && (
                    <>
                        <span className="text-gray-300">|</span>
                        <span className="flex items-center gap-1 text-blue-600">
                            <Wrench size={12} />
                            tools
                        </span>
                    </>
                )}
            </div>

            {/* Status badges */}
            <div className="flex items-center gap-2 mb-4">
                {isCached && (
                    <span className="text-xs px-2 py-0.5 rounded bg-green-100 text-green-700 flex items-center gap-1">
                        <CheckCircle size={12} />
                        Downloaded
                    </span>
                )}
                {isLoaded && (
                    <span className="text-xs px-2 py-0.5 rounded bg-blue-100 text-blue-700 flex items-center gap-1">
                        <Zap size={12} />
                        Loaded
                    </span>
                )}
                {!isCached && !isDownloading && (
                    <span className="text-xs px-2 py-0.5 rounded bg-gray-100 text-gray-500">
                        Not Downloaded
                    </span>
                )}
            </div>

            {/* Download progress */}
            {isDownloading && downloadProgress && (
                <div className="mb-4">
                    <div className="flex items-center justify-between text-xs text-gray-500 mb-1">
                        <span className="truncate max-w-[60%]">{downloadProgress.file}</span>
                        <span>{downloadProgress.progress.toFixed(1)}%</span>
                    </div>
                    <div className="w-full bg-gray-200 rounded-full h-2">
                        <div
                            className="bg-blue-500 h-2 rounded-full transition-all duration-300"
                            style={{ width: `${downloadProgress.progress}%` }}
                        />
                    </div>
                </div>
            )}

            {/* Action buttons */}
            <div className="flex items-center gap-2">
                {!isCached && !isDownloading && (
                    <button
                        onClick={onDownload}
                        className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-blue-500 text-white rounded-lg hover:bg-blue-600 transition-colors text-sm font-medium"
                    >
                        <Download size={16} />
                        Download
                    </button>
                )}
                {isCached && !isLoaded && (
                    <button
                        onClick={onRemove}
                        className="flex items-center justify-center gap-2 px-3 py-2 bg-red-50 text-red-600 rounded-lg hover:bg-red-100 transition-colors text-sm"
                    >
                        <Trash2 size={16} />
                        Remove from Cache
                    </button>
                )}
                {isLoaded && (
                    <button
                        onClick={onUnload}
                        className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-gray-100 text-gray-700 rounded-lg hover:bg-gray-200 transition-colors text-sm font-medium"
                    >
                        Unload
                    </button>
                )}
                {isDownloading && (
                    <div className="flex-1 flex items-center justify-center gap-2 px-3 py-2 bg-gray-100 text-gray-500 rounded-lg text-sm">
                        <Loader2 size={16} className="animate-spin" />
                        Downloading...
                    </div>
                )}
            </div>
        </div>
    );
}
