import { useState, useCallback } from 'react';
import type { ToolCallRecord } from '../../../store/chat-store';
import { formatMillisecondsAsDuration } from '../utils';

interface InlineToolCallResultProps {
    call: ToolCallRecord;
}

/**
 * Inline Tool Call Result - shows a single tool call result inline in the message
 * Note: For sql_select, the formatted table is shown OUTSIDE this accordion (in the main chat area)
 * This component shows the raw data inside an expandable accordion
 */
export const InlineToolCallResult = ({ call }: InlineToolCallResultProps) => {
    // Auto-expand arguments on error for easier debugging
    const [showArgs, setShowArgs] = useState(call.isError);
    const [argsText, setArgsText] = useState<string | null>(
        call.isError ? JSON.stringify(call.arguments, null, 2) : null
    );
    const [showResult, setShowResult] = useState(false);
    const [resultText, setResultText] = useState<string | null>(null);

    const handleToggleArgs = useCallback(
        (next: boolean) => {
            setShowArgs(next);
            if (next && argsText === null) {
                // Defer heavy stringify to next tick to avoid blocking render.
                const run = () => {
                    try {
                        setArgsText(JSON.stringify(call.arguments, null, 2));
                    } catch (e) {
                        setArgsText('Failed to stringify arguments');
                    }
                };
                if (typeof requestIdleCallback === 'function') {
                    requestIdleCallback(run);
                } else {
                    setTimeout(run, 0);
                }
            }
        },
        [argsText, call.arguments]
    );

    const handleToggleResult = useCallback(
        (next: boolean) => {
            setShowResult(next);
            if (next && resultText === null) {
                // Defer heavy formatting to next tick
                const run = () => {
                    try {
                        // Try to pretty-print if it's JSON
                        const parsed = JSON.parse(call.result);
                        setResultText(JSON.stringify(parsed, null, 2));
                    } catch {
                        // Not JSON, show as-is
                        setResultText(call.result);
                    }
                };
                if (typeof requestIdleCallback === 'function') {
                    requestIdleCallback(run);
                } else {
                    setTimeout(run, 0);
                }
            }
        },
        [resultText, call.result]
    );

    return (
        <details className="my-3 group/tool border border-purple-200 rounded-xl overflow-hidden bg-purple-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-purple-100/50 transition-colors select-none">
                <span className="text-purple-600 text-lg">🔧</span>
                <span className="font-medium text-purple-900 text-sm">
                    1 tool call
                </span>
                {call.isError ? (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-700">
                        1 ✗
                    </span>
                ) : (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-green-100 text-green-700">
                        1 ✓
                    </span>
                )}
                <span className="ml-auto text-xs text-purple-400 group-open/tool:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-purple-200">
                <div className="px-4 py-3 bg-white">
                    <div className="flex items-center gap-2 flex-wrap">
                        <code className="text-xs px-2 py-0.5 rounded bg-gray-100 text-gray-600">{call.server}</code>
                        <span className="text-gray-400">›</span>
                        <code className="text-sm px-2 py-1 rounded bg-purple-100 text-purple-800 font-medium">{call.tool}</code>
                        {call.isError ? (
                            <span className="text-xs px-1.5 py-0.5 rounded bg-red-100 text-red-600 ml-auto">Error</span>
                        ) : (
                            <span className="text-xs px-1.5 py-0.5 rounded bg-green-100 text-green-600 ml-auto">Success</span>
                        )}
                        {call.durationMs && (
                            <span className="text-xs text-gray-400">{formatMillisecondsAsDuration(call.durationMs)}</span>
                        )}
                    </div>
                    {/* Show arguments - always visible for errors so users can debug */}
                    <details 
                        className="mt-2" 
                        onToggle={(e) => handleToggleArgs((e.target as HTMLDetailsElement).open)}
                        open={call.isError} // Auto-expand arguments on error
                    >
                        <summary className={`text-xs cursor-pointer hover:text-gray-700 ${call.isError ? 'text-red-500 font-medium' : 'text-gray-500'}`}>
                            Arguments {call.isError && '(inspect for debugging)'}
                        </summary>
                        <pre className={`mt-1 text-xs p-2 rounded overflow-x-auto whitespace-pre-wrap ${call.isError ? 'bg-red-50 text-red-700 border border-red-200' : 'bg-gray-50 text-gray-700'}`}>
                            {showArgs || call.isError
                                ? (argsText ?? JSON.stringify(call.arguments, null, 2) ?? '(no arguments)')
                                : 'Expand to view arguments'}
                        </pre>
                    </details>
                    {/* Always show raw result in accordion (formatted table is shown outside) */}
                    <details className="mt-2" onToggle={(e) => handleToggleResult((e.target as HTMLDetailsElement).open)}>
                        <summary className={`text-xs cursor-pointer hover:text-gray-700 ${call.isError ? 'text-red-500' : 'text-gray-500'}`}>
                            {call.isError ? 'Error' : 'Response'}
                        </summary>
                        <pre className={`mt-1 text-xs p-2 rounded overflow-x-auto whitespace-pre-wrap ${call.isError ? 'bg-red-50 text-red-700' : 'bg-gray-50 text-gray-700'
                            }`}>
                            {showResult
                                ? (resultText
                                    ? (resultText.length > 2000 ? resultText.slice(0, 2000) + '\n... (truncated)' : resultText)
                                    : 'Loading response...')
                                : 'Expand to view response'}
                        </pre>
                    </details>
                </div>
            </div>
        </details>
    );
};
