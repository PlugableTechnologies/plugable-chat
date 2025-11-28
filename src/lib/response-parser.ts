/**
 * Unified Response Format Parser
 * 
 * Handles multiple model-specific response formats and normalizes them to a common structure.
 * 
 * Supported formats:
 * - gpt-oss: <|channel|>analysis<|message|>...<|end|><|channel|>final<|message|>...
 * - Phi-4-reasoning: <think>...</think>
 * - Granite: <|thinking|>...<|/thinking|>
 * - Generic: Plain text (no special formatting)
 */

export type MessagePartType = 'text' | 'think';

export interface MessagePart {
    type: MessagePartType;
    content: string;
}

/**
 * Model family identifiers for response format detection
 */
export type ModelFamily = 'gpt_oss' | 'phi' | 'gemma' | 'granite' | 'generic';

/**
 * Detect the response format from content patterns
 */
export function detectResponseFormat(content: string): ModelFamily {
    if (content.includes('<|channel|>')) {
        return 'gpt_oss';
    }
    if (content.includes('<think>')) {
        return 'phi';
    }
    if (content.includes('<|thinking|>')) {
        return 'granite';
    }
    return 'generic';
}

/**
 * Parse gpt-oss channel format:
 * <|channel|>analysis<|message|>thinking content<|end|><|channel|>final<|message|>response content
 * 
 * Also handles streaming partial states:
 * - <|channel|>analysis<|message|>partial... (no <|end|> yet)
 * - <|channel|>final<|message|>partial... (streaming final response)
 */
function parseChannelFormat(content: string): MessagePart[] {
    const parts: MessagePart[] = [];
    
    // Regex to match complete channel blocks: <|channel|>TYPE<|message|>CONTENT<|end|>
    // and incomplete ones (streaming): <|channel|>TYPE<|message|>CONTENT (no end tag)
    const channelPattern = /<\|channel\|>(\w+)<\|message\|>([\s\S]*?)(?:<\|end\|>|(?=<\|channel\|>)|$)/g;
    
    let match;
    let lastIndex = 0;
    
    while ((match = channelPattern.exec(content)) !== null) {
        // Capture any text before this channel block (edge case)
        if (match.index > lastIndex) {
            const beforeText = content.substring(lastIndex, match.index).trim();
            if (beforeText) {
                parts.push({ type: 'text', content: beforeText });
            }
        }
        
        const channelType = match[1].toLowerCase();
        const channelContent = match[2];
        
        if (channelType === 'analysis') {
            // Analysis channel = thinking content
            if (channelContent.trim()) {
                parts.push({ type: 'think', content: channelContent });
            }
        } else if (channelType === 'final') {
            // Final channel = visible response
            if (channelContent.trim()) {
                parts.push({ type: 'text', content: channelContent });
            }
        } else {
            // Unknown channel type - treat as text
            if (channelContent.trim()) {
                parts.push({ type: 'text', content: channelContent });
            }
        }
        
        lastIndex = channelPattern.lastIndex;
    }
    
    // Check for any remaining content after the last match
    if (lastIndex < content.length) {
        const remaining = content.substring(lastIndex).trim();
        // Check if it's a partial channel tag (streaming)
        if (remaining && !remaining.startsWith('<|')) {
            parts.push({ type: 'text', content: remaining });
        }
    }
    
    // If no channels found, check for partial/streaming state
    if (parts.length === 0 && content.includes('<|')) {
        // Likely streaming - just show what we have
        const cleanContent = content.replace(/<\|[^|]*\|?[^>]*>?/g, '').trim();
        if (cleanContent) {
            parts.push({ type: 'text', content: cleanContent });
        }
    }
    
    return parts;
}

/**
 * Parse Phi-4 reasoning format:
 * <think>thinking content</think>visible response
 * 
 * Also handles streaming: <think>partial... (no closing tag yet)
 */
function parseThinkFormat(content: string): MessagePart[] {
    const parts: MessagePart[] = [];
    let current = content;

    while (current.length > 0) {
        const start = current.indexOf('<think>');
        if (start === -1) {
            if (current.trim()) {
                parts.push({ type: 'text', content: current });
            }
            break;
        }

        // Text before <think>
        if (start > 0) {
            const beforeText = current.substring(0, start);
            if (beforeText.trim()) {
                parts.push({ type: 'text', content: beforeText });
            }
        }

        const rest = current.substring(start + 7); // 7 is length of <think>
        const end = rest.indexOf('</think>');

        if (end === -1) {
            // Unclosed think block (streaming)
            parts.push({ type: 'think', content: rest });
            break;
        }

        parts.push({ type: 'think', content: rest.substring(0, end) });
        current = rest.substring(end + 8); // 8 is length of </think>
    }
    
    return parts;
}

/**
 * Parse Granite thinking format:
 * <|thinking|>thinking content<|/thinking|>visible response
 * 
 * Also handles streaming: <|thinking|>partial... (no closing tag yet)
 */
function parseGraniteThinkingFormat(content: string): MessagePart[] {
    const parts: MessagePart[] = [];
    let current = content;

    while (current.length > 0) {
        const start = current.indexOf('<|thinking|>');
        if (start === -1) {
            if (current.trim()) {
                parts.push({ type: 'text', content: current });
            }
            break;
        }

        // Text before <|thinking|>
        if (start > 0) {
            const beforeText = current.substring(0, start);
            if (beforeText.trim()) {
                parts.push({ type: 'text', content: beforeText });
            }
        }

        const rest = current.substring(start + 12); // 12 is length of <|thinking|>
        const end = rest.indexOf('<|/thinking|>');

        if (end === -1) {
            // Unclosed thinking block (streaming)
            parts.push({ type: 'think', content: rest });
            break;
        }

        parts.push({ type: 'think', content: rest.substring(0, end) });
        current = rest.substring(end + 13); // 13 is length of <|/thinking|>
    }
    
    return parts;
}

/**
 * Parse plain text (no special formatting)
 */
function parsePlainText(content: string): MessagePart[] {
    if (!content.trim()) {
        return [];
    }
    return [{ type: 'text', content }];
}

/**
 * Unified message content parser that handles all model response formats.
 * 
 * @param content - The raw response content from the model
 * @param modelFamily - Optional hint about which model family produced this response.
 *                      If not provided, auto-detects based on content patterns.
 * @returns Array of MessagePart objects with normalized type and content
 */
export function parseMessageContent(content: string, modelFamily?: ModelFamily): MessagePart[] {
    // Auto-detect format if not specified
    const format = modelFamily || detectResponseFormat(content);
    
    switch (format) {
        case 'gpt_oss':
            return parseChannelFormat(content);
        case 'phi':
            return parseThinkFormat(content);
        case 'granite':
            return parseGraniteThinkingFormat(content);
        case 'gemma':
        case 'generic':
        default:
            // Check if content actually has any special format markers (auto-detect fallback)
            if (content.includes('<|channel|>')) {
                return parseChannelFormat(content);
            }
            if (content.includes('<think>')) {
                return parseThinkFormat(content);
            }
            if (content.includes('<|thinking|>')) {
                return parseGraniteThinkingFormat(content);
            }
            return parsePlainText(content);
    }
}

/**
 * Check if message has only thinking/analysis content (no visible text)
 * Used to show the "Reasoning..." indicator while the model is thinking.
 */
export function hasOnlyThinkContent(content: string): boolean {
    const parts = parseMessageContent(content);
    const textParts = parts.filter(p => p.type === 'text');
    const thinkParts = parts.filter(p => p.type === 'think');
    // Has think content but no meaningful visible text
    return thinkParts.length > 0 && textParts.every(p => !p.content.trim());
}

/**
 * Strip all special format markers from content to get clean text.
 * Useful for previews, search, etc.
 */
export function stripFormatMarkers(content: string): string {
    // Remove gpt-oss channel markers
    let result = content.replace(/<\|channel\|>\w+<\|message\|>/g, '');
    result = result.replace(/<\|end\|>/g, '');
    
    // Remove phi think markers
    result = result.replace(/<\/?think>/g, '');
    
    // Remove granite thinking markers
    result = result.replace(/<\|\/thinking\|>/g, '');
    result = result.replace(/<\|thinking\|>/g, '');
    
    return result.trim();
}

/**
 * Get just the visible/final response content, excluding thinking content.
 * Useful for generating chat previews/titles.
 */
export function getVisibleContent(content: string): string {
    const parts = parseMessageContent(content);
    return parts
        .filter(p => p.type === 'text')
        .map(p => p.content)
        .join('')
        .trim();
}

