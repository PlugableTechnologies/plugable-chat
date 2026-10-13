import type { CodeExecutionRecord } from '../../../store/chat-store';
import { formatMillisecondsAsDuration } from '../utils';
import { StatusRow } from '../StatusRow';

interface CodeExecutionBlockProps {
    executions: CodeExecutionRecord[];
}

/**
 * Code execution status row - expands to the Python code and its output
 */
export const CodeExecutionBlock = ({ executions }: CodeExecutionBlockProps) => {
    if (!executions || executions.length === 0) return null;

    const errorCount = executions.filter(e => !e.success).length;
    const successCount = executions.length - errorCount;

    return (
        <StatusRow
            label={`${executions.length} code execution${executions.length !== 1 ? 's' : ''}`}
            details={[successCount > 0 && `${successCount} succeeded`, errorCount > 0 && `${errorCount} failed`]}
            tone={errorCount > 0 ? 'error' : 'neutral'}
        >
            <div className="space-y-3">
                {executions.map((exec) => (
                    <div key={exec.id}>
                        <div className={exec.success ? 'text-gray-500' : 'text-red-500'}>
                            {exec.success ? 'Succeeded' : 'Failed'}
                            {exec.durationMs ? ` · ${formatMillisecondsAsDuration(exec.durationMs)}` : ''}
                            {exec.innerToolCalls.length > 0
                                ? ` · ${exec.innerToolCalls.length} inner tool${exec.innerToolCalls.length !== 1 ? 's' : ''}: ${exec.innerToolCalls
                                    .map((call) => `${call.tool}${call.isError ? ' (failed)' : ''}`)
                                    .join(', ')}`
                                : ''}
                        </div>
                        <details className="mt-1" open>
                            <summary className="cursor-pointer text-gray-400 hover:text-gray-600">
                                Code ({exec.code.length} line{exec.code.length !== 1 ? 's' : ''})
                            </summary>
                            <pre className="mt-1 overflow-x-auto font-mono text-gray-700">{exec.code.join('\n')}</pre>
                        </details>
                        {exec.stdout && (
                            <details className="mt-1">
                                <summary className="cursor-pointer text-gray-400 hover:text-gray-600">stdout</summary>
                                <pre className="mt-1 overflow-x-auto whitespace-pre-wrap text-gray-600">{exec.stdout}</pre>
                            </details>
                        )}
                        {exec.stderr && (
                            <details className="mt-1">
                                <summary className="cursor-pointer text-red-500 hover:text-red-700">stderr</summary>
                                <pre className="mt-1 overflow-x-auto whitespace-pre-wrap text-red-600">{exec.stderr}</pre>
                            </details>
                        )}
                    </div>
                ))}
            </div>
        </StatusRow>
    );
};
