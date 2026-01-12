import type { AttachedTool } from '../../../store/chat-store';

interface AttachedToolPillsProps {
    tools: AttachedTool[];
    alwaysOnTools?: AttachedTool[];
    onRemove: (key: string) => void;
}

/**
 * Attached Tool Pills Component (supports both always-on/locked and removable pills)
 */
export const AttachedToolPills = ({
    tools,
    alwaysOnTools = [],
    onRemove
}: AttachedToolPillsProps) => {
    if (tools.length === 0 && alwaysOnTools.length === 0) return null;

    return (
        <div className="tool-pill-bar flex flex-wrap gap-2 px-2 py-2 max-w-[900px] mx-auto">
            {/* Always-on tools - locked appearance, no remove button */}
            {alwaysOnTools.map((tool) => (
                <div
                    key={`always-on-${tool.key}`}
                    className="tool-pill-locked inline-flex items-center gap-1.5 px-3 py-1.5 bg-purple-50 border border-purple-200 text-purple-700 rounded-full text-xs font-medium"
                    title={`${tool.name} on ${tool.server} (always-on)`}
                >
                    <span>🔒</span>
                    <span>{tool.name}</span>
                </div>
            ))}
            {/* Removable tools */}
            {tools.map((tool) => (
                <div
                    key={tool.key}
                    className="tool-pill inline-flex items-center gap-1.5 px-3 py-1.5 bg-purple-100 text-purple-800 rounded-full text-xs font-medium group"
                    title={`${tool.name} on ${tool.server}`}
                >
                    <span>🔧</span>
                    <span>{tool.name}</span>
                    <button
                        onClick={() => onRemove(tool.key)}
                        className="w-4 h-4 flex items-center justify-center rounded-full hover:bg-purple-200 text-purple-600 hover:text-purple-900 transition-colors"
                        title={`Remove ${tool.name}`}
                    >
                        ×
                    </button>
                </div>
            ))}
        </div>
    );
};
