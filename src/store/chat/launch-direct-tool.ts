// --direct-tool: the launch prompts go to an MCP tool through the same call the Model / Tool drop-down uses, so an
// unattended run needs no local model. Kept free of the store's full type so it can be unit tested.
import { refFromLabel, toolFromLabel, type DirectTool, type DirectToolRef } from '../../lib/direct-tools.ts';

export const LAUNCH_TOOL_WAIT_MS = 3 * 60 * 1000;
export const LAUNCH_TOOL_POLL_MS = 1000;

export interface LaunchDirectToolState {
    launchDirectToolLabel: string | null;
    directTools: DirectTool[];
    fetchDirectTools: () => Promise<void>;
    sendLaunchPrompt: () => Promise<void>;
}

/** Ask for the tool list until a server offers `label`; null once `timeoutMs` has passed. */
export async function waitForToolByLabel(
    get: () => Pick<LaunchDirectToolState, 'directTools' | 'fetchDirectTools'>,
    label: string,
    timeoutMs = LAUNCH_TOOL_WAIT_MS,
    pollMs = LAUNCH_TOOL_POLL_MS,
): Promise<DirectTool | null> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
        await get().fetchDirectTools();
        const found = toolFromLabel(get().directTools, label);
        if (found) return found;
        if (Date.now() >= deadline) return null;
        await new Promise((resolve) => setTimeout(resolve, pollMs));
    }
}

/**
 * Resolve the tool, remember it as this launch's direct tool, and send the launch prompt. A tool that never appears is
 * still sent to: the backend answers "no longer connected" and the run records that failure instead of waiting out its timer.
 */
export async function runDirectToolLaunch(
    get: () => LaunchDirectToolState,
    set: (partial: Record<string, unknown>) => void,
    timeoutMs = LAUNCH_TOOL_WAIT_MS,
    pollMs = LAUNCH_TOOL_POLL_MS,
): Promise<void> {
    const label = get().launchDirectToolLabel;
    if (!label) return;
    const found = await waitForToolByLabel(get, label, timeoutMs, pollMs);
    const ref: DirectToolRef | null = found ? { server_id: found.server_id, tool_name: found.tool_name } : refFromLabel(label);
    // State only, never saved: a launch flag must not change what the next launch starts with.
    set({ launchDirectToolRef: ref, ...(found ? { chosenDirectTool: ref } : {}) });
    await get().sendLaunchPrompt();
}
