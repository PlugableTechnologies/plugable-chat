import { memo, useMemo, useCallback, type JSX } from 'react';
import { parseMessageContent } from '../../../lib/response-parser';
import type { Message } from '../../../store/chat-store';
import { parseSqlQueryResult, stripHarmonyTokens, stripOpenAITokens } from '../utils';
import { ThinkingIndicator } from '../indicators';
import { RagContextBlock, CodeExecutionBlock } from '../rag';
import { ToolProcessingBlock, InlineToolCallResult, SqlResultTable } from '../tools';
import { MarkdownContent } from '../markdown';
import { StatusRow } from '../StatusRow';

export interface AssistantMessageProps {
    message: Message;
    isLastMessage: boolean;
    thinkingStartTime: number | null;
    toolProcessingStartTime: number | null;
    previousSystemPromptText?: string | null;
    isStreaming?: boolean;
}

export const AssistantMessage = memo(function AssistantMessage({
    message,
    isLastMessage,
    thinkingStartTime,
    toolProcessingStartTime,
    previousSystemPromptText,
    isStreaming = false,
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
                    <StatusRow key={`think-${idx}`} label="Reasoning">
                        <div className="whitespace-pre-wrap italic">{part.content || 'Thinking...'}</div>
                    </StatusRow>
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
                nodes.push(
                    <MarkdownContent
                        key={`text-${idx}`}
                        content={part.content}
                        isStreaming={isStreaming && idx === parsedParts.length - 1}
                    />
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
    }, [parsedParts, toolCalls, hasPythonToolCalls, fallbackInsertAfter, shouldInlineFallbackToolCalls, isStreaming]);

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
                <StatusRow label="System prompt" details={[message.model, `${systemPromptLength} chars`]}>
                    <button
                        onClick={() => message.systemPromptText && navigator.clipboard?.writeText(message.systemPromptText)}
                        className="mb-1 text-gray-400 underline hover:text-gray-600"
                    >
                        Copy
                    </button>
                    <MarkdownContent content={message.systemPromptText} className="!text-xs !leading-5" />
                </StatusRow>
            )}
            {message.ragChunks && message.ragChunks.length > 0 && (
                <RagContextBlock chunks={message.ragChunks} />
            )}
            {renderedParts}
            {(pythonOutputToShow || fallbackAnswer) && (
                <MarkdownContent
                    content={stripHarmonyTokens(stripOpenAITokens(pythonOutputToShow || fallbackAnswer))}
                    className="mt-2"
                />
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
