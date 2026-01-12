interface RagFilePillsProps {
    files: string[];
    alwaysOnPaths?: string[];
    onRemove: (file: string) => void;
    isIndexing: boolean;
}

/**
 * RAG File Pills Component - shows indexed files above input with remove buttons
 * Supports both always-on/locked and removable pills
 */
export const RagFilePills = ({
    files,
    alwaysOnPaths = [],
    onRemove,
    isIndexing
}: RagFilePillsProps) => {
    if (files.length === 0 && alwaysOnPaths.length === 0 && !isIndexing) return null;

    // Truncate filename to first 15 chars
    const truncateFilename = (filename: string) => {
        if (filename.length <= 15) return filename;
        return filename.slice(0, 12) + '...';
    };

    return (
        <div className="rag-file-pill-bar flex flex-wrap gap-2 px-2 py-2 max-w-[900px] mx-auto">
            {isIndexing && (
                <div className="rag-indexing-pill inline-flex items-center gap-1.5 px-3 py-1.5 bg-blue-100 text-blue-700 rounded-full text-xs font-medium">
                    <div className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse" />
                    <span>Indexing...</span>
                </div>
            )}
            {/* Always-on RAG paths - locked appearance, no remove button */}
            {alwaysOnPaths.map((path) => (
                <div
                    key={`always-on-${path}`}
                    className="rag-file-pill-locked inline-flex items-center gap-1.5 px-3 py-1.5 bg-emerald-50 border border-emerald-200 text-emerald-700 rounded-full text-xs font-medium"
                    title={`${path} (always-on)`}
                >
                    <span>🔒</span>
                    <span>{truncateFilename(path)}</span>
                </div>
            ))}
            {/* Removable files */}
            {files.map((file) => (
                <div
                    key={file}
                    className="rag-file-pill inline-flex items-center gap-1.5 px-3 py-1.5 bg-emerald-100 text-emerald-700 rounded-full text-xs font-medium group"
                    title={file}
                >
                    <span>📄</span>
                    <span>{truncateFilename(file)}</span>
                    <button
                        onClick={() => onRemove(file)}
                        className="w-4 h-4 flex items-center justify-center rounded-full hover:bg-emerald-200 text-emerald-600 hover:text-emerald-800 transition-colors"
                        title={`Remove ${file}`}
                    >
                        ×
                    </button>
                </div>
            ))}
        </div>
    );
};
