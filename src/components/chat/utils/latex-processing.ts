// LaTeX processing utilities for ChatArea
import { stripOpenAITokens, stripHarmonyTokens } from './message-formatting';

// Common LaTeX commands that indicate math content
const LATEX_MATH_COMMANDS = [
    'frac', 'sqrt', 'sum', 'prod', 'int', 'oint', 'lim', 'infty',
    'alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta', 'eta', 'theta',
    'iota', 'kappa', 'lambda', 'mu', 'nu', 'xi', 'pi', 'rho', 'sigma',
    'tau', 'upsilon', 'phi', 'chi', 'psi', 'omega',
    'Alpha', 'Beta', 'Gamma', 'Delta', 'Epsilon', 'Zeta', 'Eta', 'Theta',
    'Iota', 'Kappa', 'Lambda', 'Mu', 'Nu', 'Xi', 'Pi', 'Rho', 'Sigma',
    'Tau', 'Upsilon', 'Phi', 'Chi', 'Psi', 'Omega',
    'times', 'div', 'pm', 'mp', 'cdot', 'ast', 'star', 'circ',
    'leq', 'geq', 'neq', 'approx', 'equiv', 'sim', 'simeq', 'cong',
    'subset', 'supset', 'subseteq', 'supseteq', 'in', 'notin', 'ni',
    'cup', 'cap', 'setminus', 'emptyset', 'varnothing',
    'forall', 'exists', 'nexists', 'neg', 'land', 'lor', 'implies', 'iff',
    'partial', 'nabla', 'degree',
    'sin', 'cos', 'tan', 'cot', 'sec', 'csc', 'arcsin', 'arccos', 'arctan',
    'sinh', 'cosh', 'tanh', 'coth',
    'log', 'ln', 'exp', 'min', 'max', 'arg', 'det', 'dim', 'ker', 'hom',
    'left', 'right', 'bigl', 'bigr', 'Bigl', 'Bigr',
    'vec', 'hat', 'bar', 'dot', 'ddot', 'tilde', 'overline', 'underline',
    'overbrace', 'underbrace',
    'text', 'textbf', 'textit', 'textrm', 'mathrm', 'mathbf', 'mathit',
    'mathbb', 'mathcal', 'mathscr', 'mathfrak',
    'boxed', 'cancel', 'bcancel', 'xcancel',
    'begin', 'end', 'matrix', 'pmatrix', 'bmatrix', 'vmatrix', 'cases',
    'hspace', 'vspace', 'quad', 'qquad', 'space',
    'displaystyle', 'textstyle', 'scriptstyle',
];

// Build regex pattern for detecting LaTeX commands
const LATEX_COMMAND_PATTERN = new RegExp(
    `\\\\(${LATEX_MATH_COMMANDS.join('|')})(?![a-zA-Z])`,
    'g'
);

/**
 * Wrap undelimited LaTeX expressions in $ delimiters
 * This handles cases where the model outputs LaTeX without proper math delimiters
 */
export const wrapUndelimitedLatex = (content: string): string => {
    // Early exit: skip if content doesn't have any LaTeX commands
    if (!content.includes('\\')) {
        return content;
    }

    // Early exit: skip if content looks like JSON/code (prevents backtracking)
    const looksLikeJson = /^\s*[\[{]/.test(content) && /"[^"]*"\s*:/.test(content);
    if (looksLikeJson) {
        return content;
    }

    // Track positions that are already in math mode or code
    const mathRanges: [number, number][] = [];
    const codeRanges: [number, number][] = [];

    // Find existing math delimiters ($$...$$ and $...$)
    let match;
    const displayMathRegex = /\$\$[\s\S]*?\$\$/g;
    while ((match = displayMathRegex.exec(content)) !== null) {
        mathRanges.push([match.index, match.index + match[0].length]);
    }

    const inlineMathRegex = /\$(?!\$)[^\$\n]+\$(?!\$)/g;
    while ((match = inlineMathRegex.exec(content)) !== null) {
        mathRanges.push([match.index, match.index + match[0].length]);
    }

    // Find code blocks and inline code
    const codeBlockRegex = /```[\s\S]*?```/g;
    while ((match = codeBlockRegex.exec(content)) !== null) {
        codeRanges.push([match.index, match.index + match[0].length]);
    }

    const inlineCodeRegex = /`[^`\n]+`/g;
    while ((match = inlineCodeRegex.exec(content)) !== null) {
        codeRanges.push([match.index, match.index + match[0].length]);
    }

    // Check if a position is inside math or code
    const isProtected = (pos: number): boolean => {
        return mathRanges.some(([start, end]) => pos >= start && pos < end) ||
            codeRanges.some(([start, end]) => pos >= start && pos < end);
    };

    // Find and wrap undelimited LaTeX expressions
    // Pattern matches: LaTeX command followed by more math content
    // e.g., \frac{4}{3} \pi r^3 or V = \frac{a}{b}
    const latexExpressionRegex = /(?:^|[^\\$])((\\(?:frac|sqrt|sum|prod|int|lim)\s*\{[^}]*\}\s*(?:\{[^}]*\})?|\\(?:text|textbf|textit|mathrm|mathbf)\s*\{[^}]*\})(?:\s*[+\-*/=^_]?\s*(?:\\[a-zA-Z]+(?:\s*\{[^}]*\})*|[a-zA-Z0-9.]+|\{[^}]*\}|[+\-*/=^_]))*)/g;

    const replacements: { start: number; end: number; text: string }[] = [];

    while ((match = latexExpressionRegex.exec(content)) !== null) {
        const fullMatch = match[1];
        const startPos = match.index + match[0].indexOf(fullMatch);

        // Skip if this position is already in math or code
        if (isProtected(startPos)) continue;

        // Only wrap if it contains actual LaTeX commands
        if (LATEX_COMMAND_PATTERN.test(fullMatch)) {
            replacements.push({
                start: startPos,
                end: startPos + fullMatch.length,
                text: `$${fullMatch.trim()}$`
            });
        }

        // Reset the regex lastIndex to avoid infinite loops
        LATEX_COMMAND_PATTERN.lastIndex = 0;
    }

    // Also catch simpler patterns: standalone LaTeX commands with arguments
    // e.g., \times 10^{27} or \approx 1.41
    const simpleLatexRegex = /(?:^|[\s(=])((\\(?:times|approx|equiv|leq|geq|neq|pm|mp|cdot|div|infty|pi|alpha|beta|gamma|delta|theta|lambda|mu|sigma|omega|phi|psi|partial|nabla|sum|prod|int)\b)(?:\s*[0-9.]+)?(?:\s*\\times\s*[0-9.]+)?(?:\s*\^[\s{]*[-0-9]+\}?)?(?:\s*\\text\{[^}]*\})?)/g;

    while ((match = simpleLatexRegex.exec(content)) !== null) {
        const fullMatch = match[1];
        const startPos = match.index + match[0].indexOf(fullMatch);

        if (isProtected(startPos)) continue;

        // Check it's not already inside our planned replacements
        const overlaps = replacements.some(r =>
            (startPos >= r.start && startPos < r.end) ||
            (startPos + fullMatch.length > r.start && startPos + fullMatch.length <= r.end)
        );

        if (!overlaps) {
            replacements.push({
                start: startPos,
                end: startPos + fullMatch.length,
                text: `$${fullMatch.trim()}$`
            });
        }
    }

    // Sort replacements by position (descending) to apply from end to start
    replacements.sort((a, b) => b.start - a.start);

    // Apply replacements
    let result = content;
    for (const { start, end, text } of replacements) {
        result = result.slice(0, start) + text + result.slice(end);
    }

    return result;
};

/**
 * Convert LaTeX bracket/paren delimiters to dollar signs for remark-math
 */
export const convertLatexDelimiters = (content: string): string => {
    // Early exit: skip expensive processing if content looks like JSON/code
    // This prevents catastrophic backtracking on JSON arrays/objects
    const looksLikeJson = /^\s*[\[{]/.test(content) && /"[^"]*"\s*:/.test(content);
    if (looksLikeJson) {
        return content;
    }

    let result = content;

    // Convert \[...\] to $$...$$ (display math)
    // Use a non-greedy match to handle multiple blocks
    result = result.replace(/\\\[([\s\S]*?)\\\]/g, (_match, inner) => {
        return `$$${inner}$$`;
    });

    // Convert \(...\) to $...$ (inline math)
    result = result.replace(/\\\(([\s\S]*?)\\\)/g, (_match, inner) => {
        return `$${inner}$`;
    });

    // Handle bare brackets [ ... ] that contain LaTeX (has backslash commands)
    // Be careful not to match markdown links or array-like content
    // Only match if the content has LaTeX patterns like \frac, \text, \times, etc.
    // FIXED: Use atomic group simulation to prevent catastrophic backtracking
    // Instead of complex regex, use simple bracket matching then check content
    let bracketIdx = 0;
    while ((bracketIdx = result.indexOf('[', bracketIdx)) !== -1) {
        // Skip if preceded by another bracket (like [[)
        if (bracketIdx > 0 && result[bracketIdx - 1] === '[') {
            bracketIdx++;
            continue;
        }
        // Find matching closing bracket (simple, no nesting for LaTeX)
        const closeIdx = result.indexOf(']', bracketIdx + 1);
        if (closeIdx === -1) break;
        // Skip if followed by ( (markdown link)
        if (result[closeIdx + 1] === '(') {
            bracketIdx = closeIdx + 1;
            continue;
        }
        const inner = result.substring(bracketIdx + 1, closeIdx);
        // Only convert if it has LaTeX commands and isn't too long (avoid JSON arrays)
        const hasLatexCommands = /\\[a-zA-Z]+/.test(inner);
        const hasMathPatterns = /[_^{}]/.test(inner) && /\\[a-zA-Z]/.test(inner);
        const isTooLong = inner.length > 500; // Avoid converting large JSON arrays
        if ((hasLatexCommands || hasMathPatterns) && !isTooLong) {
            const replacement = `$$${inner.trim()}$$`;
            result = result.substring(0, bracketIdx) + replacement + result.substring(closeIdx + 1);
            bracketIdx += replacement.length;
        } else {
            bracketIdx = closeIdx + 1;
        }
    }

    // Handle bare parentheses ( ... ) that contain LaTeX
    // Be more conservative here since parentheses are common
    // FIXED: Use simple iteration instead of backtracking regex
    let parenIdx = 0;
    while ((parenIdx = result.indexOf('(', parenIdx)) !== -1) {
        const closeIdx = result.indexOf(')', parenIdx + 1);
        if (closeIdx === -1) break;
        const inner = result.substring(parenIdx + 1, closeIdx);
        // Check for LaTeX command (backslash followed by letters)
        const hasLatexCommand = /\\[a-zA-Z]{2,}/.test(inner);
        // Also check for subscript/superscript patterns common in math
        const hasMathNotation = /[_^]/.test(inner) && /\\/.test(inner);
        // Scientific notation pattern
        const hasScientificNotation = /\\times\s*10\s*\^/.test(inner);
        // Exclude things that look like file paths or are too long
        const looksLikePath = /^\/[a-zA-Z]/.test(inner.trim());
        const isTooLong = inner.length > 500;

        if ((hasLatexCommand || hasMathNotation || hasScientificNotation) && !looksLikePath && !isTooLong) {
            const replacement = `$${inner.trim()}$`;
            result = result.substring(0, parenIdx) + replacement + result.substring(closeIdx + 1);
            parenIdx += replacement.length;
        } else {
            parenIdx = closeIdx + 1;
        }
    }

    // NEW: Wrap undelimited LaTeX expressions in inline math delimiters
    // This catches cases where LaTeX commands appear in plain text without any delimiters
    result = wrapUndelimitedLatex(result);

    return result;
};

/**
 * Helper to wrap raw \boxed{} in math delimiters to ensure they render
 * Also strips special tokens and converts LaTeX delimiters
 */
export const preprocessLaTeX = (content: string): string => {
    // First strip OpenAI tokens, then any leftover harmony tokens
    let processed = stripOpenAITokens(content);
    processed = stripHarmonyTokens(processed);

    // Then convert LaTeX delimiters
    processed = convertLatexDelimiters(processed);

    // Now handle \boxed{} and other special cases
    let result = '';
    let i = 0;

    // States
    let inMath: false | '$' | '$$' = false;
    let inCode: false | '`' | '```' = false;

    while (i < processed.length) {
        // 1. Handle Code Blocks
        if (!inMath && !inCode && processed.startsWith('```', i)) {
            inCode = '```';
            result += '```';
            i += 3;
            continue;
        }
        if (!inMath && inCode === '```' && processed.startsWith('```', i)) {
            inCode = false;
            result += '```';
            i += 3;
            continue;
        }

        // 2. Handle Inline Code
        if (!inMath && !inCode && processed.startsWith('`', i)) {
            inCode = '`';
            result += '`';
            i += 1;
            continue;
        }
        if (!inMath && inCode === '`' && processed.startsWith('`', i)) {
            inCode = false;
            result += '`';
            i += 1;
            continue;
        }

        // If in code, just consume
        if (inCode) {
            result += processed[i];
            i++;
            continue;
        }

        // 3. Handle Math Delimiters
        // Escaped dollar? \$
        if (processed.startsWith('\\$', i)) {
            result += '\\$';
            i += 2;
            continue;
        }

        if (processed.startsWith('$$', i)) {
            if (inMath === '$$') inMath = false;
            else if (!inMath) inMath = '$$';
            result += '$$';
            i += 2;
            continue;
        }
        if (processed.startsWith('$', i)) {
            // Check if this is a currency amount ($ followed by digit)
            const nextChar = processed[i + 1];
            const isCurrency = nextChar && /[0-9]/.test(nextChar);

            if (isCurrency && !inMath) {
                // Escape the dollar sign so it renders literally
                result += '\\$';
                i += 1;
                continue;
            }

            if (inMath === '$') inMath = false;
            else if (!inMath) inMath = '$';
            result += '$';
            i += 1;
            continue;
        }

        // 4. Handle \boxed{
        if (!inMath && processed.startsWith('\\boxed{', i)) {
            // Look ahead to find matching brace
            let braceCount = 1;
            let ptr = i + 7; // skip \boxed{

            while (ptr < processed.length && braceCount > 0) {
                if (processed[ptr] === '\\') {
                    ptr += 2; // skip escaped char
                    continue;
                }
                if (processed[ptr] === '{') braceCount++;
                if (processed[ptr] === '}') braceCount--;
                ptr++;
            }

            if (braceCount === 0) {
                // Found complete block - extract the inner content
                const innerContent = processed.substring(i + 7, ptr - 1);

                // Check if content contains LaTeX commands like \text{}, \mathbf{}, etc.
                const hasLatexCommands = /\\[a-zA-Z]+\{/.test(innerContent);

                // Check if content looks like plain prose text (has spaces, no math operators, no LaTeX commands)
                const looksLikePlainText = !hasLatexCommands &&
                    innerContent.includes(' ') &&
                    !/[+\-*/=^_{}\\]/.test(innerContent);

                if (looksLikePlainText) {
                    // For plain text content (no LaTeX commands), use HTML box
                    result += '<div style="border: 2px solid #2e2e2e; padding: 0.5em 0.75em; border-radius: 6px; margin: 0.5em 0; display: inline-block; max-width: 100%; word-wrap: break-word;">' + innerContent + '</div>';
                } else {
                    // Content has LaTeX commands or math - let KaTeX handle it
                    result += '$\\boxed{' + innerContent + '}$';
                }
                i = ptr;
                continue;
            }
            // If not found (unclosed), fall through to default char handling
        }

        result += processed[i];
        i++;
    }
    return result;
};
