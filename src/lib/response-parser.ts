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

/**
 * Strip OpenAI special tokens that may leak through from models
 * These include: <|start|>, <|end|>, <|im_start|>, <|im_end|>, <|endoftext|>, etc.
 * Also handles role markers like <|start|>assistant, <|im_start|>user, etc.
 */
function stripOpenAITokens(content: string): string {
    return content
        .replace(/<\|(?:start|end|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>(?:assistant|user|system)?/gi, '')
        .replace(/<\|(?:start|end|im_start|im_end|endoftext|eot_id|begin_of_text|end_of_text)\|>/gi, '')
        // Clean up any leftover newlines at the start from removed tokens
        .replace(/^\s*\n+/, '');
}

export type MessagePartType = 'text' | 'think' | 'tool_call';

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
 * Check if a JSON string looks like a tool call
 * Tool calls have "name" field with a known tool name pattern
 */
function looksLikeToolCallJson(jsonStr: string): boolean {
    // #region agent log
    const startTs = Date.now();
    // #endregion
    // Known tool names or structural markers that should be treated as tool calls
    const toolPatterns = [
        '"name"\\s*:\\s*"python_execution"',
        '"name"\\s*:\\s*"code_execution"',
        '"name"\\s*:\\s*"tool_search"',
        '"name"\\s*:\\s*"schema_search"',
        '"name"\\s*:\\s*"sql_select"',
        '"tool_name"\\s*:',
        '"tool"\\s*:\\s*"',
        '"name"\\s*:\\s*"[^"]+___',  // MCP tool format: server___tool
    ];
    
    // Check if it contains name/arguments structure typical of tool calls
    const hasToolStructure = (/"name"\s*:\s*"/.test(jsonStr) || /"tool_name"\s*:\s*"/.test(jsonStr) || /"tool"\s*:\s*"/.test(jsonStr)) && 
                            (/"arguments"\s*:/.test(jsonStr) || /"parameters"\s*:/.test(jsonStr) || /"tool_args"\s*:/.test(jsonStr) || /"code"\s*:/.test(jsonStr) || /"args"\s*:/.test(jsonStr));
    
    if (!hasToolStructure) return false;
    
    // Check against known patterns or if it has a server/tool pair
    const hasServerToolPair = /"server"\s*:\s*"/.test(jsonStr) && /"tool"\s*:\s*"/.test(jsonStr);
    
    const result = hasServerToolPair || toolPatterns.some(pattern => new RegExp(pattern, 'i').test(jsonStr));
    // #region agent log
    const durMs = Date.now() - startTs;
    if (durMs > 20 || jsonStr.length > 5000) {
        fetch('http://127.0.0.1:7243/ingest/94c42ad2-8d49-47ca-bf15-e6e37a3ccd05',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H2',location:'response-parser.ts:looksLikeToolCallJson',message:'looksLikeToolCallJson_duration',data:{jsonLen:jsonStr.length,durationMs:durMs,hasServerToolPair,result},timestamp:Date.now()})}).catch(()=>{});
    }
    // #endregion
    return result;
}

/**
 * Extract tool_call blocks from text content.
 * Handles:
 * - <tool_call>...</tool_call> (standard format)
 * - <function_call>...</function_call> (Granite format)
 * - ```json { "name": "...", "arguments": {...} } ``` (markdown code blocks)
 * - Unclosed tags during streaming
 * 
 * Returns an array of MessageParts with tool_call extracted from text.
 */
function extractToolCalls(parts: MessagePart[]): MessagePart[] {
    // #region agent log
    const startTs = Date.now();
    const totalLen = parts.reduce((acc, p) => acc + p.content.length, 0);
    // #endregion
    const result: MessagePart[] = [];
    
    for (const part of parts) {
        if (part.type !== 'text') {
            // Keep non-text parts as-is
            result.push(part);
            continue;
        }
        
        let current = part.content;
        
        while (current.length > 0) {
            // Find the next tool_call, function_call tag, or JSON code block
            const toolCallStart = current.indexOf('<tool_call>');
            const functionCallStart = current.indexOf('<function_call>');
            
            // Also look for markdown JSON code blocks that might be tool calls
            const jsonCodeBlockMatch = current.match(/```(?:json)?\s*\n?\s*(\{[\s\S]*?(?:"name"|"tool_name"|"tool"|"server")\s*:[\s\S]*?)\n?\s*```/);
            const jsonCodeBlockStart = jsonCodeBlockMatch ? current.indexOf(jsonCodeBlockMatch[0]) : -1;
            
            // Check for unclosed JSON code block (streaming)
            const unclosedJsonMatch = !jsonCodeBlockMatch ? current.match(/```(?:json)?\s*\n?\s*(\{[\s\S]*?(?:"name"|"tool_name"|"tool"|"server")\s*:[\s\S]*)$/) : null;
            const unclosedJsonStart = unclosedJsonMatch ? current.indexOf(unclosedJsonMatch[0]) : -1;
            
            // Determine which comes first
            let tagStart = -1;
            let tagType: 'tool_call' | 'function_call' | 'json_block' | 'json_block_unclosed' | null = null;
            let openTagLen = 0;
            let closeTag = '';
            
            // Find the earliest match
            const candidates = [
                { start: toolCallStart, type: 'tool_call' as const, openLen: 11, close: '</tool_call>' },
                { start: functionCallStart, type: 'function_call' as const, openLen: 15, close: '</function_call>' },
                { start: jsonCodeBlockStart, type: 'json_block' as const, openLen: 0, close: '' },
                { start: unclosedJsonStart, type: 'json_block_unclosed' as const, openLen: 0, close: '' },
            ].filter(c => c.start !== -1);
            
            if (candidates.length > 0) {
                const earliest = candidates.reduce((a, b) => a.start < b.start ? a : b);
                tagStart = earliest.start;
                tagType = earliest.type;
                openTagLen = earliest.openLen;
                closeTag = earliest.close;
            }
            
            if (tagStart === -1) {
                // No more tool calls - add remaining text
                if (current.trim()) {
                    result.push({ type: 'text', content: current });
                }
                break;
            }
            
            // Add text before the tag
            if (tagStart > 0) {
                const beforeText = current.substring(0, tagStart);
                if (beforeText.trim()) {
                    result.push({ type: 'text', content: beforeText });
                }
            }
            
            // Handle JSON code blocks specially
            if (tagType === 'json_block' && jsonCodeBlockMatch) {
                const jsonContent = jsonCodeBlockMatch[1];
                // Only treat as tool_call if it looks like a tool call JSON
                if (looksLikeToolCallJson(jsonContent)) {
                    result.push({ type: 'tool_call', content: jsonContent });
                } else {
                    // Not a tool call, keep as text
                    result.push({ type: 'text', content: jsonCodeBlockMatch[0] });
                }
                current = current.substring(tagStart + jsonCodeBlockMatch[0].length);
                continue;
            }
            
            if (tagType === 'json_block_unclosed' && unclosedJsonMatch) {
                const jsonContent = unclosedJsonMatch[1];
                // Only treat as tool_call if it looks like a tool call JSON
                if (looksLikeToolCallJson(jsonContent)) {
                    result.push({ type: 'tool_call', content: jsonContent });
                } else {
                    result.push({ type: 'text', content: unclosedJsonMatch[0] });
                }
                break; // Unclosed means end of content
            }
            
            // Handle standard XML-style tags
            const rest = current.substring(tagStart + openTagLen);
            const endIdx = rest.indexOf(closeTag);
            
            if (endIdx === -1) {
                // Unclosed tag (streaming) - capture everything as tool_call
                result.push({ type: 'tool_call', content: rest });
                break;
            }
            
            // Extract the tool_call content
            const toolContent = rest.substring(0, endIdx);
            result.push({ type: 'tool_call', content: toolContent });
            
            // Continue with the rest of the content
            current = rest.substring(endIdx + closeTag.length);
        }
    }
    
    // #region agent log
    const durMs = Date.now() - startTs;
    if (durMs > 50 || totalLen > 5000) {
        fetch('http://127.0.0.1:7243/ingest/94c42ad2-8d49-47ca-bf15-e6e37a3ccd05',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H1',location:'response-parser.ts:extractToolCalls',message:'extractToolCalls_duration',data:{totalLen,partsCount:parts.length,durationMs:durMs},timestamp:Date.now()})}).catch(()=>{});
    }
    // #endregion
    return result;
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
    // #region agent log
    const startTs = Date.now();
    // #endregion
    // First, strip any leaked OpenAI special tokens
    const cleanedContent = stripOpenAITokens(content);
    
    // Auto-detect format if not specified
    const format = modelFamily || detectResponseFormat(cleanedContent);
    
    let parts: MessagePart[];
    
    switch (format) {
        case 'gpt_oss':
            parts = parseChannelFormat(cleanedContent);
            break;
        case 'phi':
            parts = parseThinkFormat(cleanedContent);
            break;
        case 'granite':
            parts = parseGraniteThinkingFormat(cleanedContent);
            break;
        case 'gemma':
        case 'generic':
        default:
            // Check if content actually has any special format markers (auto-detect fallback)
            if (cleanedContent.includes('<|channel|>')) {
                parts = parseChannelFormat(cleanedContent);
            } else if (cleanedContent.includes('<think>')) {
                parts = parseThinkFormat(cleanedContent);
            } else if (cleanedContent.includes('<|thinking|>')) {
                parts = parseGraniteThinkingFormat(cleanedContent);
            } else {
                parts = parsePlainText(cleanedContent);
            }
            break;
    }
    
    // Post-process to extract tool_call blocks from text parts
    // This handles <tool_call> and <function_call> tags that can appear in any format
    const result = extractToolCalls(parts);
    // #region agent log
    const durMs = Date.now() - startTs;
    if (durMs > 50 || content.length > 3000) {
        fetch('http://127.0.0.1:7243/ingest/94c42ad2-8d49-47ca-bf15-e6e37a3ccd05',{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({sessionId:'debug-session',runId:'pre-fix',hypothesisId:'H4',location:'response-parser.ts:parseMessageContent',message:'parseMessageContent_duration',data:{contentLen:content.length,partsCount:result.length,format,durationMs:durMs},timestamp:Date.now()})}).catch(()=>{});
    }
    // #endregion
    return result;
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
 * Check if message has only tool_call content (no visible text)
 * Used to show the "Processing tool..." indicator while tools are executing.
 */
export function hasOnlyToolCallContent(content: string): boolean {
    const parts = parseMessageContent(content);
    const textParts = parts.filter(p => p.type === 'text');
    const toolCallParts = parts.filter(p => p.type === 'tool_call');
    // Has tool_call content but no meaningful visible text
    return toolCallParts.length > 0 && textParts.every(p => !p.content.trim());
}

/**
 * Check if message content contains any tool_call parts
 */
export function hasToolCallContent(content: string): boolean {
    const parts = parseMessageContent(content);
    return parts.some(p => p.type === 'tool_call');
}

/**
 * Strip all special format markers from content to get clean text.
 * Useful for previews, search, etc.
 */
export function stripFormatMarkers(content: string): string {
    // First strip OpenAI special tokens
    let result = stripOpenAITokens(content);
    
    // Remove gpt-oss channel markers
    result = result.replace(/<\|channel\|>\w+<\|message\|>/g, '');
    result = result.replace(/<\|end\|>/g, '');
    
    // Remove phi think markers
    result = result.replace(/<\/?think>/g, '');
    
    // Remove granite thinking markers
    result = result.replace(/<\|\/thinking\|>/g, '');
    result = result.replace(/<\|thinking\|>/g, '');
    
    // Remove tool_call markers and their content
    result = result.replace(/<tool_call>[\s\S]*?<\/tool_call>/g, '');
    result = result.replace(/<function_call>[\s\S]*?<\/function_call>/g, '');
    // Handle unclosed tags (streaming)
    result = result.replace(/<tool_call>[\s\S]*$/g, '');
    result = result.replace(/<function_call>[\s\S]*$/g, '');
    
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

