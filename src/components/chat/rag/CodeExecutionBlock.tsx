import type { CodeExecutionRecord } from '../../../store/chat-store';
import { formatMillisecondsAsDuration } from '../utils';

interface CodeExecutionBlockProps {
    executions: CodeExecutionRecord[];
}

/**
 * Collapsible Code Execution Block - shows Python code execution
 */
export const CodeExecutionBlock = ({ executions }: CodeExecutionBlockProps) => {
    if (!executions || executions.length === 0) return null;

    const errorCount = executions.filter(e => !e.success).length;
    const successCount = executions.length - errorCount;

    return (
        <details className="my-4 group/code border border-blue-200 rounded-xl overflow-hidden bg-blue-50/50">
            <summary className="cursor-pointer px-4 py-3 flex items-center gap-3 hover:bg-blue-100/50 transition-colors select-none">
                <span className="text-blue-600 text-lg">🐍</span>
                <span className="font-medium text-blue-900 text-sm">
                    {executions.length} code execution{executions.length !== 1 ? 's' : ''}
                </span>
                {successCount > 0 && (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-green-100 text-green-700">
                        {successCount} ✓
                    </span>
                )}
                {errorCount > 0 && (
                    <span className="text-xs px-1.5 py-0.5 rounded-full bg-red-100 text-red-700">
                        {errorCount} ✗
                    </span>
                )}
                <span className="ml-auto text-xs text-blue-400 group-open/code:rotate-180 transition-transform">▼</span>
            </summary>
            <div className="border-t border-blue-200 divide-y divide-blue-100">
                {executions.map((exec) => (
                    <div key={exec.id} className="px-4 py-3 bg-white">
                        <div className="flex items-center gap-2 mb-2">
                            {exec.success ? (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-green-100 text-green-600">Success</span>
                            ) : (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-red-100 text-red-600">Error</span>
                            )}
                            <span className="text-xs text-gray-400">{formatMillisecondsAsDuration(exec.durationMs)}</span>
                            {exec.innerToolCalls.length > 0 && (
                                <span className="text-xs px-1.5 py-0.5 rounded bg-purple-100 text-purple-600">
                                    {exec.innerToolCalls.length} inner tool{exec.innerToolCalls.length !== 1 ? 's' : ''}
                                </span>
                            )}
                        </div>
                        <details className="mt-2" open>
                            <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                Code ({exec.code.length} line{exec.code.length !== 1 ? 's' : ''})
                            </summary>
                            <pre className="mt-1 text-xs bg-gray-900 text-gray-100 p-3 rounded overflow-x-auto font-mono">
                                {exec.code.join('\n')}
                            </pre>
                        </details>
                        {exec.stdout && (
                            <details className="mt-2">
                                <summary className="text-xs text-green-600 cursor-pointer hover:text-green-700">
                                    stdout
                                </summary>
                                <pre className="mt-1 text-xs bg-green-50 text-green-800 p-2 rounded overflow-x-auto whitespace-pre-wrap">
                                    {exec.stdout}
                                </pre>
                            </details>
                        )}
                        {exec.stderr && (
                            <details className="mt-2">
                                <summary className="text-xs text-red-500 cursor-pointer hover:text-red-700">
                                    stderr
                                </summary>
                                <pre className="mt-1 text-xs bg-red-50 text-red-700 p-2 rounded overflow-x-auto whitespace-pre-wrap">
                                    {exec.stderr}
                                </pre>
                            </details>
                        )}
                        {exec.innerToolCalls.length > 0 && (
                            <div className="mt-3 pl-3 border-l-2 border-purple-200">
                                <p className="text-xs text-purple-600 mb-2 font-medium">Inner Tool Calls:</p>
                                <div className="space-y-2">
                                    {exec.innerToolCalls.map((call) => (
                                        <div key={call.id} className="bg-purple-50 rounded p-2 text-xs">
                                            <div className="flex items-center gap-2">
                                                <code className="text-purple-700 font-medium">{call.tool}</code>
                                                {call.isError ? (
                                                    <span className="text-red-500">✗</span>
                                                ) : (
                                                    <span className="text-green-500">✓</span>
                                                )}
                                            </div>
                                        </div>
                                    ))}
                                </div>
                            </div>
                        )}
                    </div>
                ))}
            </div>
        </details>
    );
};
