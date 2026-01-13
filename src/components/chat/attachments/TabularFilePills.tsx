import type { AttachedTabularFile } from '../../../store/chat/types';

interface TabularFilePillsProps {
    files: AttachedTabularFile[];
    onRemove: (filePath: string) => void;
}

/**
 * Tabular File Pills Component - shows attached CSV/TSV/Excel files
 * Displays file name, row count, and column preview
 */
export const TabularFilePills = ({
    files,
    onRemove,
}: TabularFilePillsProps) => {
    if (files.length === 0) return null;

    // Truncate filename to first 15 chars
    const truncateFilename = (filename: string) => {
        if (filename.length <= 15) return filename;
        return filename.slice(0, 12) + '...';
    };

    // Format row count for display
    const formatRowCount = (count: number) => {
        if (count >= 1000000) return `${(count / 1000000).toFixed(1)}M`;
        if (count >= 1000) return `${(count / 1000).toFixed(1)}K`;
        return count.toString();
    };

    return (
        <div className="tabular-file-pill-bar flex flex-wrap gap-2 px-2 py-2 max-w-[900px] mx-auto">
            {files.map((file) => (
                <div
                    key={file.filePath}
                    className="tabular-file-pill inline-flex items-center gap-1.5 px-3 py-1.5 bg-purple-100 text-purple-700 rounded-full text-xs font-medium group"
                    title={`${file.fileName} - ${file.rowCount} rows, ${file.headers.length} columns (headers${file.variableIndex}/rows${file.variableIndex})`}
                >
                    <span>📊</span>
                    <span>{truncateFilename(file.fileName)}</span>
                    <span className="text-purple-500">
                        ({formatRowCount(file.rowCount)} rows)
                    </span>
                    <button
                        onClick={() => onRemove(file.filePath)}
                        className="w-4 h-4 flex items-center justify-center rounded-full hover:bg-purple-200 text-purple-600 hover:text-purple-800 transition-colors"
                        title={`Remove ${file.fileName}`}
                    >
                        ×
                    </button>
                </div>
            ))}
        </div>
    );
};
