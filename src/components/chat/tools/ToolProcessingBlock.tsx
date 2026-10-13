import { useEffect, useState, useMemo } from 'react';
import { parseMessageContent } from '../../../lib/response-parser';
import { parseToolCallJsonFromContent, type ParsedToolCallInfo } from '../utils';
import { formatSecondsAsTime } from '../utils';
import { StatusRow } from '../StatusRow';

interface ToolProcessingBlockProps {
    content: string;
    startTime: number;
}

/**
 * Tool processing block (shown inline in message when only tool_call content exists)
 * Shows a quiet status row with tool call details and processing status
 */
export const ToolProcessingBlock = ({ content, startTime }: ToolProcessingBlockProps) => {
    const [elapsed, setElapsed] = useState(0);
    const [showRaw, setShowRaw] = useState(false);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    const parsedData = useMemo(() => {
        // Avoid expensive parsing for very large payloads; show raw toggle instead.
        if (content.length > 80000) {
            return { parsedCalls: [] as ParsedToolCallInfo[], rawContent: content, oversized: true };
        }
        const parts = parseMessageContent(content);
        const toolCallParts = parts.filter(p => p.type === 'tool_call');
        const parsedCalls = toolCallParts
            .map(part => parseToolCallJsonFromContent(part.content))
            .filter((call): call is ParsedToolCallInfo => call !== null);
        const rawToolContent = toolCallParts.map(p => p.content).join('\n\n');
        return { parsedCalls, rawContent: rawToolContent, oversized: false };
    }, [content]);

    const { parsedCalls, rawContent, oversized } = parsedData;
    const elapsedText = elapsed >= 1 ? formatSecondsAsTime(elapsed) : null;

    if (parsedCalls.length === 0) {
        // Fallback with expandable raw content if we can't parse the tool calls
        return (
            <StatusRow label="Preparing tool call" details={[elapsedText]} isLive>
                {oversized && !showRaw ? (
                    <div>
                        Large tool payload omitted for performance.{' '}
                        <button className="underline" onClick={() => setShowRaw(true)}>
                            Show anyway
                        </button>
                    </div>
                ) : rawContent ? (
                    <pre className="overflow-x-auto whitespace-pre-wrap">{rawContent}</pre>
                ) : (
                    <p className="italic">Tool call content is being streamed...</p>
                )}
            </StatusRow>
        );
    }

    return (
        <StatusRow
            label={`Running ${parsedCalls.length} tool call${parsedCalls.length !== 1 ? 's' : ''}`}
            details={[parsedCalls.map((call) => call.tool).join(', '), elapsedText]}
            isLive
        >
            {parsedCalls.map((call, idx) => (
                <div key={idx} className="mb-1">
                    <div className="font-mono">{call.server} › {call.tool}</div>
                    {Object.keys(call.arguments).length > 0 && (
                        <pre className="mt-0.5 overflow-x-auto whitespace-pre-wrap">
                            {JSON.stringify(call.arguments, null, 2)}
                        </pre>
                    )}
                </div>
            ))}
        </StatusRow>
    );
};
