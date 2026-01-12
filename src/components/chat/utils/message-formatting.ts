// Message formatting utilities for ChatArea

/**
 * Format elapsed time in seconds to human-readable string
 */
export const formatSecondsAsTime = (seconds: number): string => {
    if (seconds < 60) return `${seconds}s`;
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}m ${secs}s`;
};

/**
 * Format milliseconds to human-readable duration
 */
export const formatMillisecondsAsDuration = (ms?: number): string => {
    if (!ms) return '';
    if (ms < 1000) return `${ms}ms`;
    const seconds = Math.floor(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}m ${secs}s`;
};

/**
 * Strip OpenAI special tokens that may leak through
 * NOTE: Do NOT strip <|end|> here - harmony format uses it as a channel terminator
 * and the response-parser needs it to properly extract channel boundaries.
 * Leftover <|end|> tokens after parsing are stripped by stripHarmonyTokens below.
 */
export const stripOpenAITokens = (content: string): string => {
    // Remove common OpenAI special tokens (but NOT <|end|> which harmony format needs)
    // Patterns: <|start|>, <|im_start|>, <|im_end|>, <|endoftext|>
    // Also handles role markers like <|start|>assistant, <|im_start|>user, etc.
    return content
        .replace(/<\|(?:start|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>(?:assistant|user|system)?/gi, '')
        .replace(/<\|(?:start|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>/gi, '')
        // Clean up any leftover newlines at the start from removed tokens
        .replace(/^\n+/, '');
};

/**
 * Strip harmony-specific tokens after parsing (for clean rendering)
 */
export const stripHarmonyTokens = (content: string): string => {
    return content
        .replace(/<\|channel\|>\w+(?:\s+to=\S+)?(?:\s+<\|constrain\|>\w+)?<\|message\|>/gi, '')
        .replace(/<\|(?:end|call|return)\|>/gi, '')
        .replace(/^\n+/, '');
};
