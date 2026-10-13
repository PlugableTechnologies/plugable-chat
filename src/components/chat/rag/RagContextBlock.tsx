import type { RagChunk } from '../../../store/chat-store';
import { StatusRow } from '../StatusRow';

interface RagContextBlockProps {
    chunks: RagChunk[];
}

/** Status line listing the document chunks used as context; expands to the chunks. */
export const RagContextBlock = ({ chunks }: RagContextBlockProps) => {
    if (!chunks || chunks.length === 0) return null;

    const uniqueFiles = [...new Set(chunks.map(c => c.source_file))];

    return (
        <StatusRow
            label={`${chunks.length} document chunk${chunks.length !== 1 ? 's' : ''} used`}
            details={[`${uniqueFiles.length} file${uniqueFiles.length !== 1 ? 's' : ''}`]}
        >
            <div className="space-y-2">
                {chunks.map((chunk, idx) => (
                    <div key={chunk.id || idx}>
                        <div className="font-mono text-gray-500">
                            {chunk.source_file} · {(chunk.score * 100).toFixed(0)}% match
                        </div>
                        <p className="mt-0.5 whitespace-pre-wrap text-gray-500">{chunk.content}</p>
                    </div>
                ))}
            </div>
        </StatusRow>
    );
};
