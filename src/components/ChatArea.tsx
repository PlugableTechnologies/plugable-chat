/**
 * DEPRECATED: This file is maintained for backward compatibility.
 * Import from './chat' directly for the modular chat implementation.
 *
 * The ChatArea has been split into sub-components:
 * - ChatArea.tsx - Main orchestration component
 * - indicators/ - ThinkingIndicator, SearchingIndicator, ToolExecutionIndicator
 * - messages/ - AssistantMessage, UserMessage
 * - tools/ - ToolApprovalDialog, ToolProcessingBlock, InlineToolCallResult, SqlResultTable
 * - rag/ - RagContextBlock, CodeExecutionBlock
 * - attachments/ - RagFilePills, AttachedTablePills, AttachedToolPills, AttachmentMenu
 * - input/ - InputBar
 * - modals/ - DatabaseAttachmentModal, ToolAttachmentModal
 * - utils/ - latex-processing, message-formatting, tool-parsing
 *
 * See src/components/chat/index.ts for the full export list.
 */
export { ChatArea } from './chat';
