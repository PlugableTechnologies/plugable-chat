import type { RagChunk } from '../../../store/chat-store';

interface RagContextBlockProps {
    chunks: RagChunk[];
}

/**
 * Collapsible RAG Context Block - shows document chunks used as context
 */
export const RagContextBlock = ({ chunks }: RagContextBlockProps) => {
    if (!chunks || chunks.length === 0) return null;

    // Get unique source files
    const uniqueFiles = [...new Set(chunks.map(c => c.source_file))];

    return (
        <details className="my-4 group/rag border border-emerald-200 rounded-xl overflow-hidden bg-emerald-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-emerald-100/50 transition-colors select-none">
                <span className="text-emerald-600 text-lg">📚</span>
                <span className="font-medium text-emerald-900 text-sm">
                    {chunks.length} document chunk{chunks.length !== 1 ? 's' : ''} used
                </span>
                <span className="text-xs px-1.5 py-0.5 rounded-full bg-emerald-100 text-emerald-700">
                    {uniqueFiles.length} file{uniqueFiles.length !== 1 ? 's' : ''}
                </span>
                <span className="ml-auto text-xs text-emerald-400 group-open/rag:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-emerald-200 divide-y divide-emerald-100">
                {chunks.map((chunk, idx) => (
                    <div key={chunk.id || idx} className="px-4 py-3 bg-white">
                        <div className="flex items-center gap-2 flex-wrap">
                            <span className="text-emerald-500">📄</span>
                            <code className="text-xs px-2 py-0.5 rounded bg-emerald-100 text-emerald-700 font-medium">
                                {chunk.source_file}
                            </code>
                            <span className="text-xs px-1.5 py-0.5 rounded bg-gray-100 text-gray-600 ml-auto">
                                {(chunk.score * 100).toFixed(0)}% match
                            </span>
                        </div>
                        <p className="mt-2 text-xs text-gray-600 italic whitespace-pre-wrap">
                            "{chunk.content}"
                        </p>
                    </div>
                ))}
            </div>
        </details>
    );
};
