import { useState, useCallback } from 'react';
import type { ToolCallRecord } from '../../../store/chat-store';
import { formatMillisecondsAsDuration } from '../utils';
import { StatusRow } from '../StatusRow';

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
        <StatusRow
            label={call.isError ? 'Tool failed' : 'Used 1 tool'}
            details={[call.tool, call.durationMs ? formatMillisecondsAsDuration(call.durationMs) : null]}
            tone={call.isError ? 'error' : 'neutral'}
        >
            <div className="font-mono text-gray-500">
                {call.server} › {call.tool}
            </div>
            {/* Arguments stay open on errors so users can debug */}
            <details
                className="mt-1"
                onToggle={(e) => handleToggleArgs((e.target as HTMLDetailsElement).open)}
                open={call.isError}
            >
                <summary className="cursor-pointer text-gray-400 hover:text-gray-600">Arguments</summary>
                <pre className="mt-1 overflow-x-auto whitespace-pre-wrap text-gray-600">
                    {showArgs || call.isError
                        ? (argsText ?? JSON.stringify(call.arguments, null, 2) ?? '(no arguments)')
                        : 'Expand to view arguments'}
                </pre>
            </details>
            {/* Raw result lives here; the formatted table is shown outside */}
            <details className="mt-1" onToggle={(e) => handleToggleResult((e.target as HTMLDetailsElement).open)}>
                <summary className={`cursor-pointer hover:text-gray-600 ${call.isError ? 'text-red-500' : 'text-gray-400'}`}>
                    {call.isError ? 'Error' : 'Response'}
                </summary>
                <pre className={`mt-1 overflow-x-auto whitespace-pre-wrap ${call.isError ? 'text-red-600' : 'text-gray-600'}`}>
                    {showResult
                        ? (resultText
                            ? (resultText.length > 2000 ? resultText.slice(0, 2000) + '\n... (truncated)' : resultText)
                            : 'Loading response...')
                        : 'Expand to view response'}
                </pre>
            </details>
        </StatusRow>
    );
};
