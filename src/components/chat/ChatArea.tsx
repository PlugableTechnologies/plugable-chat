import { useEffect, useRef, useState, useCallback } from 'react';
import {
    useChatStore,
    getModelStateMessage,
    generateClientChatIdentifier,
    deriveChatTitleFromPrompt,
    deriveChatPreviewFromMessage
} from '../../store/chat-store';
import { useSettingsStore } from '../../store/settings-store';
import { StatusBar, StreamingWarningBar, ModelStateBar, StartupStateBar } from '../StatusBar';
import { invoke } from '../../lib/api';
import { hasOnlyThinkContent, hasOnlyToolCallContent } from '../../lib/response-parser';

// Helper to log to backend terminal for debugging
const logToBackend = (message: string) => {
    invoke('log_to_terminal', { message }).catch(() => {});
};

// Sub-components
import { SearchingIndicator, ToolExecutionIndicator } from './indicators';
import { AssistantMessage } from './messages';
import { ToolApprovalDialog } from './tools';
import { RagFilePills, AttachedTablePills, AttachedToolPills } from './attachments';
import { InputBar } from './input';
import { DatabaseAttachmentModal, ToolAttachmentModal } from './modals';

/**
 * Main ChatArea component - orchestrates chat UI
 */
export function ChatArea() {
    const {
        chatMessages,
        chatInputValue,
        setChatInputValue,
        appendChatMessage,
        assistantStreamingActive,
        setAssistantStreamingActive,
        stopActiveChatGeneration,
        currentChatId,
        reasoningEffort,
        triggerRelevanceSearch, clearRelevanceSearch, isConnecting,
        // RAG state
        attachedPaths, ragIndexedFiles, isIndexingRag,
        addAttachment, searchRagContext, clearRagContext, removeRagFile,
        // Attachment state
        attachedDatabaseTables, attachedTools,
        removeAttachedTable, removeAttachedTool,
        clearAttachedTables, clearAttachedTools,
        // Always-on state (synced from settings)
        alwaysOnTools, alwaysOnTables, alwaysOnRagPaths, syncAlwaysOnFromSettings,
        // Tool execution state
        pendingToolApproval, toolExecution, approveCurrentToolCall, rejectCurrentToolCall,
        // Streaming state
        streamingChatId,
        // Model state machine (deterministic sync with backend)
        modelState, isModelReady
    } = useChatStore();

    const { settings, openSettings, setActiveTab } = useSettingsStore();

    // Check if streaming is active in a different chat (input should be blocked)
    const isStreamingInOtherChat = streamingChatId !== null && streamingChatId !== currentChatId;
    
    // Model state blocking: block prompts when model is not ready (loading, switching, etc.)
    const isModelBlocking = !isModelReady;
    const textareaRef = useRef<HTMLTextAreaElement>(null);
    const messagesEndRef = useRef<HTMLDivElement>(null);
    const [thinkingStartTime, setThinkingStartTime] = useState<number | null>(null);
    const [toolProcessingStartTime, setToolProcessingStartTime] = useState<number | null>(null);
    // Local RAG state for UI (controlled directly, not from store)
    const [ragStartTime, setRagStartTime] = useState<number | null>(null);
    const [ragStage, setRagStage] = useState<'indexing' | 'searching'>('indexing');
    const [isRagProcessing, setIsRagProcessing] = useState(false);

    // Per-chat attachment modal state
    const [dbModalOpen, setDbModalOpen] = useState(false);
    const [toolModalOpen, setToolModalOpen] = useState(false);

    // Mutual exclusivity
    const hasRagAttachments = attachedPaths.length > 0 || ragIndexedFiles.length > 0;
    const hasDbAttachments = attachedDatabaseTables.length > 0;
    const filesDisabled = hasDbAttachments;
    const dbDisabled = hasRagAttachments;

    // Follow mode: auto-scroll to bottom when true, stop when user scrolls up
    const scrollContainerRef = useRef<HTMLDivElement>(null);
    const [isFollowMode, setIsFollowMode] = useState(true);

    // Scroll handler to detect when user scrolls away from bottom
    const handleScroll = useCallback(() => {
        const container = scrollContainerRef.current;
        if (!container) return;
        const { scrollTop, scrollHeight, clientHeight } = container;
        const atBottom = scrollHeight - scrollTop - clientHeight < 50;
        setIsFollowMode(atBottom);
    }, []);

    // Sync always-on state from settings when component mounts or settings change
    useEffect(() => {
        syncAlwaysOnFromSettings();
    }, [settings, syncAlwaysOnFromSettings]);

    // Track when thinking phase starts
    useEffect(() => {
        const lastMessage = chatMessages[chatMessages.length - 1];
        const isThinkingOnly = lastMessage?.role === 'assistant' &&
            hasOnlyThinkContent(lastMessage.content) &&
            assistantStreamingActive;

        if (isThinkingOnly && !thinkingStartTime) {
            setThinkingStartTime(Date.now());
        } else if (!assistantStreamingActive || (lastMessage?.role === 'assistant' && !hasOnlyThinkContent(lastMessage.content))) {
            setThinkingStartTime(null);
        }
    }, [chatMessages, assistantStreamingActive, thinkingStartTime]);

    // Track when tool processing phase starts (only tool_call content, no visible text)
    useEffect(() => {
        const lastMessage = chatMessages[chatMessages.length - 1];
        const isToolProcessingOnly = lastMessage?.role === 'assistant' &&
            hasOnlyToolCallContent(lastMessage.content) &&
            assistantStreamingActive;

        if (isToolProcessingOnly && !toolProcessingStartTime) {
            setToolProcessingStartTime(Date.now());
        } else if (!assistantStreamingActive || (lastMessage?.role === 'assistant' && !hasOnlyToolCallContent(lastMessage.content))) {
            setToolProcessingStartTime(null);
        }
    }, [chatMessages, assistantStreamingActive, toolProcessingStartTime]);

    // Reset follow mode when switching chats
    useEffect(() => {
        setIsFollowMode(true);
    }, [currentChatId]);

    // Auto-scroll to bottom (only when in follow mode)
    useEffect(() => {
        if (isFollowMode) {
            messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
        }
    }, [chatMessages, assistantStreamingActive, isFollowMode]);

    // Auto-resize textarea (ChatGPT-style: starts compact, grows as you type)
    useEffect(() => {
        if (textareaRef.current) {
            // Reset height to auto to get accurate scrollHeight
            textareaRef.current.style.height = 'auto';
            // Set height to scrollHeight, capped by CSS max-height
            const newHeight = Math.max(32, Math.min(textareaRef.current.scrollHeight, 200));
            textareaRef.current.style.height = `${newHeight}px`;
        }
    }, [chatInputValue]);

    // Trigger relevance search as user types (debounced in store)
    useEffect(() => {
        if (chatInputValue.trim().length >= 3) {
            triggerRelevanceSearch(chatInputValue);
        } else {
            clearRelevanceSearch();
        }
    }, [chatInputValue, triggerRelevanceSearch, clearRelevanceSearch]);

    // Handle file selection via Tauri dialog
    const handleAttachFiles = async () => {
        try {
            const { open } = await import('@tauri-apps/plugin-dialog');
            // Get default directory from backend (test-data for development)
            const defaultPath = await invoke<string | null>('get_test_data_directory');
            const selected = await open({
                multiple: true,
                defaultPath: defaultPath || undefined,
                filters: [{
                    name: 'Documents',
                    extensions: ['txt', 'csv', 'tsv', 'md', 'json', 'pdf', 'docx']
                }]
            });
            if (selected) {
                const paths = Array.isArray(selected) ? selected : [selected];
                // Process each file sequentially (addAttachment now triggers immediate indexing)
                for (const path of paths) {
                    if (path) await addAttachment(path);
                }
            }
        } catch (e) {
            console.error('[ChatArea] Failed to open file dialog:', e);
        }
    };

    // Handle folder selection via Tauri dialog
    const handleAttachFolder = async () => {
        try {
            const { open } = await import('@tauri-apps/plugin-dialog');
            // Get default directory from backend (test-data for development)
            const defaultPath = await invoke<string | null>('get_test_data_directory');
            const selected = await open({
                directory: true,
                multiple: false,
                defaultPath: defaultPath || undefined
            });
            if (selected && typeof selected === 'string') {
                await addAttachment(selected);
            }
        } catch (e) {
            console.error('[ChatArea] Failed to open folder dialog:', e);
        }
    };

    // Handle database selection modal
    const handleAttachDatabase = () => {
        const hasConnectedDatabases = (settings?.database_toolbox?.sources || []).some(s => s.enabled);
        if (!hasConnectedDatabases) {
            setActiveTab('databases');
            openSettings();
        } else {
            setDbModalOpen(true);
        }
    };

    // Handle tool selection modal
    const handleAttachTool = () => {
        setToolModalOpen(true);
    };

    // Handle clearing attachments (also clears RAG context)
    const handleClearAttachments = async () => {
        await clearRagContext();
        clearAttachedTables();
        clearAttachedTools();
    };

    const handleSend = async () => {
        const text = chatInputValue;
        if (!text.trim()) return;
        const trimmedText = text.trim();

        const storeState = useChatStore.getState();
        const isNewChat = !currentChatId;
        const chatId = isNewChat ? generateClientChatIdentifier() : currentChatId!;
        if (isNewChat) {
            storeState.setCurrentChatId(chatId);
            if (storeState.currentModel === 'Loading...') {
                try {
                    await storeState.fetchModels();
                } catch (error) {
                    console.error('[ChatArea] Failed to refresh models before sending new chat:', error);
                }
            }
        }

        const existingSummary = storeState.history.find((chat) => chat.id === chatId);
        const derivedTitle = existingSummary?.title ?? deriveChatTitleFromPrompt(trimmedText);
        const preview = deriveChatPreviewFromMessage(trimmedText);
        const summaryScore = existingSummary?.score ?? 0;
        const summaryPinned = existingSummary?.pinned ?? false;

        storeState.upsertHistoryEntry({
            id: chatId,
            title: derivedTitle,
            preview,
            score: summaryScore,
            pinned: summaryPinned,
            model: storeState.currentModel
        });

        // Add user message (show original text to user)
        appendChatMessage({
            id: Date.now().toString(),
            role: 'user',
            content: text,
            timestamp: Date.now(),
        });
        setChatInputValue('');
        clearRelevanceSearch(); // Clear relevance results when sending
        if (textareaRef.current) textareaRef.current.style.height = 'auto';
        setAssistantStreamingActive(true);
        storeState.setLastStreamActivityTs(Date.now());

        // Track which chat we're streaming to (for cross-chat switching)
        storeState.setStreamingChatId(chatId);

        // Show streaming status in status bar
        storeState.setOperationStatus({
            type: 'streaming',
            message: 'Generating response...',
            startTime: Date.now(),
        });

        // Add placeholder for assistant
        const assistantMsgId = (Date.now() + 1).toString();
        appendChatMessage({
            id: assistantMsgId,
            role: 'assistant',
            content: '',
            timestamp: Date.now(),
            model: storeState.currentModel
        });
        
        // Log state after setup - check that assistant placeholder was added
        const afterState = useChatStore.getState();
        const lastMsg = afterState.chatMessages[afterState.chatMessages.length - 1];
        logToBackend(`[FRONTEND] 📤 handleSend setup complete | chatId=${chatId.slice(0,8)} | msgCount=${afterState.chatMessages.length} | lastRole=${lastMsg?.role} | streaming=${afterState.assistantStreamingActive}`);

        try {
            // Check if we have RAG context to search (files are indexed immediately on attach)
            let messageToSend = text;
            const hasRagContext = storeState.ragIndexedFiles.length > 0;

            if (hasRagContext) {
                console.log('[ChatArea] Searching RAG context with', storeState.ragIndexedFiles.length, 'indexed files');

                // Show RAG indicator
                setIsRagProcessing(true);
                setRagStartTime(Date.now());
                setRagStage('searching');

                const allChunks = await searchRagContext(trimmedText, 10);
                // Entirely ignore rag results with a relevance score below 30%
                const relevantChunks = allChunks.filter(chunk => chunk.score >= 0.3);

                if (relevantChunks.length > 0) {
                    // Store chunks on the assistant message for display
                    useChatStore.setState((state) => {
                        const newMessages = [...state.chatMessages];
                        const lastIdx = newMessages.length - 1;
                        if (lastIdx >= 0 && newMessages[lastIdx].role === 'assistant') {
                            newMessages[lastIdx] = { ...newMessages[lastIdx], ragChunks: relevantChunks };
                        }
                        return { chatMessages: newMessages };
                    });

                    // Build context string for the model
                    const contextParts = relevantChunks.map((chunk, idx) =>
                        `[${idx + 1}] From "${chunk.source_file}" (relevance: ${(chunk.score * 100).toFixed(1)}%):\n${chunk.content}`
                    );
                    const contextString = contextParts.join('\n\n');

                    // Prepend context to the message
                    messageToSend = `Context from attached documents:\n\n${contextString}\n\n---\n\nUser question: ${text}`;
                    console.log('[ChatArea] Added', relevantChunks.length, 'chunks as context');
                }

                // Hide RAG indicator
                setIsRagProcessing(false);
                setRagStartTime(null);
            }

            // Fetch the exact system prompt that will be sent for this turn
            let systemPromptPreview: string | null = null;
            try {
                systemPromptPreview = await invoke<string>('get_system_prompt_preview', {
                    userPrompt: messageToSend,
                    attachedFiles: storeState.ragIndexedFiles,
                    attachedTables: storeState.attachedDatabaseTables.map(t => ({
                        source_id: t.sourceId,
                        table_fq_name: t.tableFqName,
                        column_count: t.columnCount,
                        schema_text: null
                    })),
                    attachedTools: storeState.attachedTools.map(t => t.key),
                });
            } catch (e) {
                console.error('[ChatArea] Failed to fetch system prompt preview:', e);
            }

            if (systemPromptPreview) {
                useChatStore.setState((state) => {
                    const newMessages = [...state.chatMessages];
                    const lastIdx = newMessages.length - 1;
                    if (lastIdx >= 0 && newMessages[lastIdx].role === 'assistant') {
                        newMessages[lastIdx] = { ...newMessages[lastIdx], systemPromptText: systemPromptPreview };
                    }
                    return { chatMessages: newMessages };
                });
            }

            const history = chatMessages.map((m) => ({
                role: m.role,
                content: m.content,
                system_prompt: m.systemPromptText,
            }));
            // Call backend - streaming will trigger events
            // Frontend is source of truth for model selection
            logToBackend(`[FRONTEND] 📞 Calling backend chat command | chatId=${chatId.slice(0,8)} | historyLen=${history.length}`);
            const returnedChatId = await invoke<string>('chat', {
                chatId,
                title: isNewChat ? derivedTitle : undefined,
                message: messageToSend,
                history: history,
                reasoningEffort,
                model: storeState.currentModel,
                attachedFiles: storeState.ragIndexedFiles,
                attachedTables: storeState.attachedDatabaseTables.map(t => ({
                    source_id: t.sourceId,
                    table_fq_name: t.tableFqName,
                    column_count: t.columnCount,
                    schema_text: null
                })),
                attachedTools: storeState.attachedTools.map(t => t.key),
            });
            logToBackend(`[FRONTEND] ✅ Backend chat command returned | returnedChatId=${returnedChatId?.slice(0,8)}`);

            if (returnedChatId && returnedChatId !== chatId) {
                storeState.setCurrentChatId(returnedChatId);
                storeState.upsertHistoryEntry({
                    id: returnedChatId,
                    title: derivedTitle,
                    preview,
                    score: summaryScore,
                    pinned: summaryPinned
                });
            }
        } catch (error) {
            console.error('[ChatArea] Failed to send message:', error);
            // Reset RAG state on error
            setIsRagProcessing(false);
            setRagStartTime(null);
            // Update the last message with error
            useChatStore.setState((state) => {
                const newMessages = [...state.chatMessages];
                const lastIdx = newMessages.length - 1;
                if (lastIdx >= 0) {
                    newMessages[lastIdx] = {
                        ...newMessages[lastIdx],
                        content: `Error: ${error}`
                    };
                }
                return { chatMessages: newMessages };
            });
            setAssistantStreamingActive(false);
        }
    };

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            handleSend();
        }
    };

    return (
        <div id="chat-area" className="chat-area h-full w-full flex flex-col text-gray-800 font-sans relative overflow-hidden">
            {/* Status Bar for model operations */}
            <StatusBar />

            {/* Startup state bar for initialization progress */}
            <StartupStateBar />

            {/* Model state bar for blocking states (loading, switching, etc.) */}
            <ModelStateBar />

            {/* Warning when streaming in another chat */}
            <StreamingWarningBar />

            {/* Scrollable Messages Area - takes all remaining space */}
            <div ref={scrollContainerRef} onScroll={handleScroll} className="chat-scroll-region flex-1 min-h-0 w-full overflow-y-auto flex flex-col px-4 sm:px-6 pt-6 pb-6">
                {chatMessages.length === 0 ? (
                    <div className="chat-empty-state flex-1 flex flex-col items-center justify-center px-6">
                        <div className="chat-empty-copy mb-8 text-center">
                            <h1 className="chat-empty-title text-2xl font-bold text-gray-900">
                                {isConnecting ? "Wait, Loading Local Models ..." : 
                                 isModelBlocking ? getModelStateMessage(modelState) :
                                 "How can I help you today?"}
                            </h1>
                        </div>
                    </div>
                ) : (
                    <div className="chat-thread w-full max-w-none space-y-6 py-0">
                        {chatMessages.map((m, idx) => {
                            const previousAssistantSystemPrompt =
                                chatMessages
                                    .slice(0, idx)
                                    .reverse()
                                    .find((prev) => prev.role === 'assistant' && prev.systemPromptText)?.systemPromptText || null;
                            return (
                                <div key={m.id} className={`chat-message-row flex w-full ${m.role === 'user' ? 'justify-end' : 'justify-start'}`}>
                                    <div
                                        className={`
                                    chat-bubble relative w-full max-w-none rounded-2xl px-5 py-3.5 text-[15px] leading-7
                                    ${m.role === 'user'
                                                ? 'chat-bubble-user bg-gray-100 text-gray-900'
                                                : 'chat-bubble-assistant bg-gray-50 text-gray-900'
                                            }
                                `}
                                    >
                                        <div className="chat-message-content prose prose-slate max-w-none break-words text-gray-900">
                                            {m.role === 'assistant' ? (
                                                <AssistantMessage
                                                    message={m}
                                                    isLastMessage={m.role === 'assistant' && chatMessages[chatMessages.length - 1]?.id === m.id}
                                                    thinkingStartTime={thinkingStartTime}
                                                    toolProcessingStartTime={toolProcessingStartTime}
                                                    previousSystemPromptText={previousAssistantSystemPrompt}
                                                />
                                            ) : (
                                                <div className="whitespace-pre-wrap">{m.content}</div>
                                            )}
                                        </div>
                                    </div>
                                </div>
                            );
                        })}
                        {assistantStreamingActive && chatMessages[chatMessages.length - 1]?.role !== 'assistant' && (
                            <div className="flex w-full justify-start">
                                <div className="bg-gray-50 rounded-2xl px-6 py-4">
                                    <div className="flex gap-1.5">
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '0ms' }} />
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '150ms' }} />
                                        <div className="w-1.5 h-1.5 bg-gray-400 rounded-full animate-bounce" style={{ animationDelay: '300ms' }} />
                                    </div>
                                </div>
                            </div>
                        )}
                        <div ref={messagesEndRef} />
                    </div>
                )}
            </div>

            {/* RAG Searching Indicator */}
            {isRagProcessing && ragStartTime && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <SearchingIndicator startTime={ragStartTime} stage={ragStage} />
                    </div>
                </div>
            )}

            {/* Tool Execution Indicator */}
            {toolExecution.currentTool && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <ToolExecutionIndicator
                            server={toolExecution.currentTool.server}
                            tool={toolExecution.currentTool.tool}
                        />
                    </div>
                </div>
            )}

            {/* Tool Approval Dialog */}
            {pendingToolApproval && (
                <div className="flex-shrink-0 px-4 sm:px-6">
                    <div className="max-w-[900px] mx-auto">
                        <ToolApprovalDialog
                            calls={pendingToolApproval.calls}
                            onApprove={approveCurrentToolCall}
                            onReject={rejectCurrentToolCall}
                        />
                    </div>
                </div>
            )}

            {/* Attachment Modals */}
            <DatabaseAttachmentModal
                isOpen={dbModalOpen}
                onClose={() => setDbModalOpen(false)}
                chatPrompt={chatInputValue}
            />
            <ToolAttachmentModal
                isOpen={toolModalOpen}
                onClose={() => setToolModalOpen(false)}
            />

            {/* Fixed Input Area at Bottom */}
            <div className="chat-input-section flex-shrink-0 mt-1 pb-4">
                {/* Attachment Pills */}
                <div className="chat-input-pill-row px-2 sm:px-6">
                    <RagFilePills
                        files={ragIndexedFiles}
                        alwaysOnPaths={alwaysOnRagPaths}
                        onRemove={removeRagFile}
                        isIndexing={isIndexingRag}
                    />
                    <AttachedTablePills
                        tables={attachedDatabaseTables}
                        alwaysOnTables={alwaysOnTables}
                        onRemove={removeAttachedTable}
                    />
                    <AttachedToolPills
                        tools={attachedTools}
                        alwaysOnTools={alwaysOnTools}
                        onRemove={removeAttachedTool}
                    />
                </div>
                <div className="chat-input-bar-row px-2 sm:px-6">
                    <InputBar
                        className=""
                        input={chatInputValue}
                        setInput={setChatInputValue}
                        handleSend={handleSend}
                        handleStop={stopActiveChatGeneration}
                        handleKeyDown={handleKeyDown}
                        textareaRef={textareaRef}
                        isLoading={assistantStreamingActive}
                        attachedCount={attachedPaths.length + attachedDatabaseTables.length + attachedTools.length}
                        onAttachFiles={handleAttachFiles}
                        onAttachFolder={handleAttachFolder}
                        onAttachDatabase={handleAttachDatabase}
                        onAttachTool={handleAttachTool}
                        onClearAttachments={handleClearAttachments}
                        filesDisabled={filesDisabled}
                        dbDisabled={dbDisabled}
                        disabled={isStreamingInOtherChat || isModelBlocking}
                    />
                </div>
            </div>
        </div>
    );
}
