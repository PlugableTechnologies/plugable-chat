// The prompts a launch runs after its first one (--prompts-file): each starts in a fresh chat
// once the previous chat has finished. Kept free of store types so it can be unit tested.

export interface LaunchPromptStep {
    nextLaunchPrompt: string | null;
    remainingLaunchPrompts: string[];
}

/** Take the next non-blank prompt off the front of the queue. */
export function takeNextLaunchPrompt(queuedLaunchPrompts: string[]): LaunchPromptStep {
    const remaining = [...queuedLaunchPrompts];
    while (remaining.length > 0) {
        const candidate = (remaining.shift() ?? '').trim();
        if (candidate) {
            return { nextLaunchPrompt: candidate, remainingLaunchPrompts: remaining };
        }
    }
    return { nextLaunchPrompt: null, remainingLaunchPrompts: [] };
}
