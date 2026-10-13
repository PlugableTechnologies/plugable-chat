/**
 * Repair partially streamed markdown so half-finished constructs render as formatting
 * instead of flashing raw markup (an open code fence, a table row cut mid-cell).
 * Apply only while a message is still streaming; finished messages render as-is.
 */
export const closeOpenMarkdownConstructs = (content: string): string => {
    let repaired = content;

    const fenceCount = (repaired.match(/^[ \t]*```/gm) || []).length;
    const insideOpenFence = fenceCount % 2 === 1;
    if (insideOpenFence) {
        return repaired.endsWith('\n') ? `${repaired}\`\`\`` : `${repaired}\n\`\`\``;
    }

    // A table row without its closing pipe is still arriving; hide it until it completes.
    const lines = repaired.split('\n');
    const lastLine = lines[lines.length - 1];
    const lastLineLooksLikePartialTableRow = /^\s*\|/.test(lastLine) && !/\|\s*$/.test(lastLine);
    if (lastLineLooksLikePartialTableRow) {
        lines.pop();
        repaired = lines.join('\n');
    }

    return repaired;
};
