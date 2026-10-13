import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeRaw from 'rehype-raw';
import rehypeSanitize, { defaultSchema } from 'rehype-sanitize';
import rehypeHighlight from 'rehype-highlight';
import type { Options as ReactMarkdownOptions } from 'react-markdown';

// Models and tools emit inline HTML (<br> inside table cells, <sup>, <details>), so raw HTML is
// parsed, then sanitized before anything else touches the tree. remark-math marks math with
// classes that the default schema would strip, so they are allowed explicitly.
const markdownSanitizeSchema = {
    ...defaultSchema,
    attributes: {
        ...defaultSchema.attributes,
        code: [...(defaultSchema.attributes?.code ?? []), ['className', /^language-./, 'math-inline', 'math-display']],
        td: [...(defaultSchema.attributes?.td ?? []), ['style', /^text-align:(left|right|center)$/]],
        th: [...(defaultSchema.attributes?.th ?? []), ['style', /^text-align:(left|right|center)$/]],
        div: [...(defaultSchema.attributes?.div ?? []), ['className', 'math', 'math-display']],
        span: [...(defaultSchema.attributes?.span ?? []), ['className', 'math', 'math-inline']],
    },
};

export const markdownRemarkPlugins: NonNullable<ReactMarkdownOptions['remarkPlugins']> = [remarkGfm, remarkMath];

export const markdownRehypePlugins: NonNullable<ReactMarkdownOptions['rehypePlugins']> = [
    rehypeRaw,
    [rehypeSanitize, markdownSanitizeSchema],
    [rehypeKatex, { throwOnError: false, errorColor: '#666666', strict: false }],
    [rehypeHighlight, { detect: false, ignoreMissing: true }],
];
