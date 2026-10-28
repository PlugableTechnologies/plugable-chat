// Which backend command one launch prompt goes through. Kept free of the store so it can be unit tested.
import type { ReasoningEffort } from './types.ts';

/**
 * The backend command and arguments for one launch prompt: the model's `chat`, or with --direct-tool the same call the
 * Model / Tool drop-down makes (`call_mcp_tool_directly`), which needs no model and records the turn the same way.
 */
export function launchTurnInvocation(
    directTool: { server_id: string; tool_name: string } | null,
    turn: { chatId: string; title: string | undefined; message: string; history: unknown[]; reasoningEffort: ReasoningEffort; model: string },
): [string, Record<string, unknown>] {
    if (directTool) {
        return ['call_mcp_tool_directly', { chatId: turn.chatId, title: turn.title, message: turn.message, history: turn.history, serverId: directTool.server_id, toolName: directTool.tool_name }];
    }
    return ['chat', {
        chatId: turn.chatId,
        title: turn.title,
        message: turn.message,
        history: turn.history,
        reasoningEffort: turn.reasoningEffort,
        model: turn.model,
        // The backend command requires all four attachment lists (ChatArea passes the real ones). Without them the
        // launch prompt failed with "missing required key attachedFiles". A launch prompt has no attachments.
        attachedFiles: [],
        attachedTables: [],
        attachedTools: [],
        attachedTabularFiles: [],
    }];
}
