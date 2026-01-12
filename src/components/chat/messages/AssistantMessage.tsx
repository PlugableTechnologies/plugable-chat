import { memo, useMemo, useCallback, type JSX } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeRaw from 'rehype-raw';
import 'katex/dist/katex.min.css';
import { parseMessageContent } from '../../../lib/response-parser';
import type { Message } from '../../../store/chat-store';
import { preprocessLaTeX, parseSqlQueryResult } from '../utils';
import { ThinkingIndicator } from '../indicators';
import { RagContextBlock, CodeExecutionBlock } from '../rag';
import { ToolProcessingBlock, InlineToolCallResult, SqlResultTable } from '../tools';

export interface AssistantMessageProps {
    message: Message;
    isLastMessage: boolean;
    thinkingStartTime: number | null;
    toolProcessingStartTime: number | null;
    previousSystemPromptText?: string | null;
}

export const AssistantMessage = memo(function AssistantMessage({
    message,
    isLastMessage,
    thinkingStartTime,
    toolProcessingStartTime,
    previousSystemPromptText,
}: AssistantMessageProps) {
    const toolCalls = message.toolCalls || [];
    const parsedParts = useMemo(() => parseMessageContent(message.content), [message.content]);
    const textParts = useMemo(() => parsedParts.filter((p) => p.type === 'text'), [parsedParts]);
    const toolCallParts = useMemo(() => parsedParts.filter((p) => p.type === 'tool_call'), [parsedParts]);
    const pythonToolCalls = useMemo(
        () => toolCalls.filter((call) => call.tool === 'python_execution'),
        [toolCalls]
    );
    const hasPythonToolCalls = pythonToolCalls.length > 0;
    const toolCallPartIndices = useMemo(
        () => parsedParts.map((p, idx) => (p.type === 'tool_call' ? idx : -1)).filter((idx) => idx !== -1),
        [parsedParts]
    );
    const lastThinkIndex = useMemo(
        () => parsedParts.reduce((last, part, idx) => (part.type === 'think' ? idx : last), -1),
        [parsedParts]
    );
    const hasAnyText = useMemo(
        () => textParts.some((part) => part.content.trim().length > 0),
        [textParts]
    );
    const isCodeOnlyBlock = useCallback((text: string) => {
        const trimmed = text.trim();
        return /^```[\s\S]*```$/.test(trimmed);
    }, []);
    const textAllCodeOnly = useMemo(
        () => textParts.length > 0 && textParts.every((part) => isCodeOnlyBlock(part.content)),
        [textParts, isCodeOnlyBlock]
    );
    const hasVisibleText = hasAnyText && !textAllCodeOnly;
    const shouldInlineFallbackToolCalls = toolCalls.length > toolCallPartIndices.length;
    const fallbackInsertAfter = shouldInlineFallbackToolCalls
        ? toolCallPartIndices.length > 0
            ? toolCallPartIndices[toolCallPartIndices.length - 1]
            : lastThinkIndex !== -1
                ? lastThinkIndex
                : parsedParts.length > 0
                    ? 0
                    : -1
        : -1;
    const latestCodeExecutionStdout = useMemo(
        () =>
            message.codeExecutions
                ?.slice()
                .reverse()
                .find((exec) => exec.stdout && exec.stdout.trim().length > 0)
                ?.stdout.trim(),
        [message.codeExecutions]
    );
    const latestPythonStdout = useMemo(
        () =>
            pythonToolCalls
                .slice()
                .reverse()
                // Only show successful python output in the main chat area
                // Errors stay in the tool accordion for the model to retry
                .find((call) => !call.isError && call.result && call.result.trim().length > 0)
                ?.result.trim(),
        [pythonToolCalls]
    );
    const latestNonPythonToolResult = useMemo(
        () =>
            toolCalls
                .slice()
                .reverse()
                // Only show successful tool results as fallback answers
                // Errors stay in the tool accordion for the model to retry
                // Exclude sql_select - SQL results are already rendered as a table in renderedParts
                .find((call) =>
                    call.tool !== 'python_execution' &&
                    call.tool !== 'sql_select' &&
                    !call.isError &&
                    call.result &&
                    call.result.trim().length > 0
                )
                ?.result.trim(),
        [toolCalls]
    );
    const pythonOutputToShow = latestPythonStdout || latestCodeExecutionStdout || '';
    const fallbackAnswer = !hasVisibleText ? latestNonPythonToolResult || '' : '';
    const showSystemPromptAccordion = useMemo(() => {
        const prompt = message.systemPromptText;
        if (!prompt || !prompt.trim()) return false;
        return prompt !== previousSystemPromptText;
    }, [message.systemPromptText, previousSystemPromptText]);
    const systemPromptLength = message.systemPromptText?.length || 0;
    const renderedParts = useMemo(() => {
        let toolCallIndex = 0;
        const nodes: JSX.Element[] = [];

        parsedParts.forEach((part, idx) => {
            if (part.type === 'think') {
                nodes.push(
                    <details key={`think-${idx}`} className="mb-4 group">
                        <summary className="cursor-pointer text-xs font-medium text-gray-400 hover:text-gray-600 select-none flex items-center gap-2 mb-2">
                            <span className="h-px flex-1 bg-gray-200 group-open:bg-gray-300 transition-colors"></span>
                            <span className="text-sm group-open:rotate-180 transition-transform inline-block">▼</span>
                        </summary>
                        <div className="pl-3 border-l-2 border-gray-300 text-gray-600 text-sm italic bg-gray-50 p-3 rounded-r-lg">
                            {part.content || "Thinking..."}
                        </div>
                    </details>
                );
                return;
            }

            if (part.type === 'tool_call') {
                const toolCallRecord = toolCalls[toolCallIndex];
                if (toolCallRecord) {
                    // For sql_select, show accordion THEN formatted table as a single grouped unit
                    if (toolCallRecord.tool === 'sql_select' && !toolCallRecord.isError) {
                        const sqlResult = parseSqlQueryResult(toolCallRecord.result);
                        if (sqlResult && sqlResult.success && sqlResult.columns.length > 0) {
                            // Wrap in fragment to ensure strict ordering: accordion first, table second
                            nodes.push(
                                <div key={`sql-group-${toolCallRecord.id}`} className="sql-tool-call-group">
                                    <InlineToolCallResult call={toolCallRecord} />
                                    <SqlResultTable sqlResult={sqlResult} />
                                </div>
                            );
                            toolCallIndex++;
                            return;
                        }
                    }
                    // Non-SQL tool calls or failed SQL - just show the accordion
                    nodes.push(
                        <InlineToolCallResult key={`toolcall-${toolCallRecord.id}`} call={toolCallRecord} />
                    );
                    toolCallIndex++;
                }
                return;
            }

            // Skip rendering text when Python tool calls will show outputs separately
            if (!hasPythonToolCalls) {
                const processedContent = preprocessLaTeX(part.content);
                nodes.push(
                    <ReactMarkdown
                        key={`text-${idx}`}
                        remarkPlugins={[remarkGfm, remarkMath]}
                        rehypePlugins={[
                            rehypeRaw,
                            [
                                rehypeKatex,
                                {
                                    throwOnError: false,
                                    errorColor: '#666666',
                                    strict: false,
                                },
                            ],
                        ]}
                        components={{
                            code({ inline, className, children, ...props }: any) {
                                const match = /language-(\w+)/.exec(className || '');
                                const codeContent = String(children).replace(/\n$/, '');

                                return !inline && match ? (
                                    <div className="my-4 rounded-xl overflow-hidden border border-gray-200 bg-gray-50 shadow-sm group/code">
                                        <div className="flex justify-between items-center bg-gray-100 px-3 py-2 border-b border-gray-200">
                                            <span className="text-xs text-gray-600 font-mono font-medium">{match[1]}</span>
                                            <button
                                                onClick={() => navigator.clipboard.writeText(codeContent)}
                                                className="text-xs text-gray-600 hover:text-gray-900 transition-colors px-2 py-1 hover:bg-gray-200 rounded opacity-0 group-hover/code:opacity-100"
                                            >
                                                📋
                                            </button>
                                        </div>
                                        <div className="bg-white p-4 overflow-x-auto text-sm">
                                            <code className={className} {...props}>
                                                {children}
                                            </code>
                                        </div>
                                    </div>
                                ) : (
                                    <code className={`${className} bg-gray-200 px-1.5 py-0.5 rounded text-[13px] text-gray-900 font-mono`} {...props}>
                                        {children}
                                    </code>
                                );
                            },
                        }}
                    >
                        {processedContent}
                    </ReactMarkdown>
                );
            }

            if (shouldInlineFallbackToolCalls && idx === fallbackInsertAfter && toolCallIndex < toolCalls.length) {
                for (; toolCallIndex < toolCalls.length; toolCallIndex++) {
                    const call = toolCalls[toolCallIndex];
                    nodes.push(
                        <InlineToolCallResult key={`toolcall-fallback-${call.id}`} call={call} />
                    );
                }
            }
        });

        if (shouldInlineFallbackToolCalls && parsedParts.length === 0 && toolCalls.length > 0) {
            toolCalls.forEach((call) => {
                nodes.push(<InlineToolCallResult key={`toolcall-fallback-${call.id}`} call={call} />);
            });
        }

        return nodes;
    }, [parsedParts, toolCalls, hasPythonToolCalls, fallbackInsertAfter, shouldInlineFallbackToolCalls]);

    const hasThinkOnly = useMemo(() => {
        const hasThink = parsedParts.some((p) => p.type === 'think');
        const textHasMeaning = textParts.some((p) => p.content.trim());
        return hasThink && !textHasMeaning;
    }, [parsedParts, textParts]);

    const hasToolOnly = useMemo(() => {
        const hasTool = toolCallParts.length > 0;
        const textHasMeaning = textParts.some((p) => p.content.trim());
        return hasTool && !textHasMeaning;
    }, [toolCallParts, textParts]);

    return (
        <>
            {showSystemPromptAccordion && message.systemPromptText && (
                <details className="system-prompt-accordion group/system-prompt my-3 border border-gray-200 rounded-lg overflow-hidden bg-gray-50/50">
                    <summary className="cursor-pointer px-3 py-1 flex items-center gap-2.5 hover:bg-gray-100/80 transition-colors select-none">
                        <span className="text-gray-400 text-base">🛈</span>
                        <span className="font-medium text-gray-500 text-xs">System prompt</span>
                        {message.model && (
                            <span className="text-[10px] px-1.5 py-0 rounded-full bg-blue-100 text-blue-700 font-semibold">
                                {message.model}
                            </span>
                        )}
                        <span className="text-[10px] px-1.5 py-0 rounded-full bg-gray-200 text-gray-600">
                            {systemPromptLength} chars
                        </span>
                        <span className="ml-auto text-[10px] text-gray-400 group-open/system-prompt:rotate-180 transition-transform">▼</span>
                    </summary>
                    <div className="border-t border-gray-200 px-3 py-2 bg-white">
                        <div className="flex justify-end mb-2">
                            <button
                                onClick={() => message.systemPromptText && navigator.clipboard?.writeText(message.systemPromptText)}
                                className="text-[10px] px-1.5 py-0.5 rounded border border-gray-200 text-gray-500 bg-gray-50 hover:bg-gray-100 transition-colors"
                            >
                                Copy
                            </button>
                        </div>
                        <div className="prose prose-slate max-w-none text-[13px] text-gray-700">
                            <ReactMarkdown
                                remarkPlugins={[remarkGfm, remarkMath]}
                                rehypePlugins={[
                                    rehypeRaw,
                                    [
                                        rehypeKatex,
                                        {
                                            throwOnError: false,
                                            errorColor: '#666666',
                                            strict: false,
                                        },
                                    ],
                                ]}
                            >
                                {message.systemPromptText}
                            </ReactMarkdown>
                        </div>
                    </div>
                </details>
            )}
            {message.ragChunks && message.ragChunks.length > 0 && (
                <RagContextBlock chunks={message.ragChunks} />
            )}
            {renderedParts}
            {(pythonOutputToShow || fallbackAnswer) && (
                <div className="mt-3">
                    <div className="bg-white border border-gray-200 rounded-xl px-4 py-3 text-gray-900 whitespace-pre-wrap">
                        {pythonOutputToShow || fallbackAnswer}
                    </div>
                </div>
            )}
            {thinkingStartTime && isLastMessage && hasThinkOnly && (
                <ThinkingIndicator startTime={thinkingStartTime} />
            )}
            {/* Only show processing block if we don't have results yet */}
            {toolProcessingStartTime && isLastMessage && hasToolOnly && toolCalls.length === 0 && (
                <ToolProcessingBlock content={message.content} startTime={toolProcessingStartTime} />
            )}
            {message.codeExecutions && message.codeExecutions.length > 0 && (
                <CodeExecutionBlock executions={message.codeExecutions} />
            )}
        </>
    );
});
