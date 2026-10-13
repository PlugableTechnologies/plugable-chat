import { memo, useMemo, type ComponentProps, type ReactNode } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import { openUrl } from '@tauri-apps/plugin-opener';
import 'katex/dist/katex.min.css';
import 'highlight.js/styles/github.css';
import { preprocessLaTeX } from '../utils/latex-processing';
import { closeOpenMarkdownConstructs } from '../utils/markdown-streaming';
import { markdownRehypePlugins, markdownRemarkPlugins } from './markdown-config';

interface HastNodeLike {
    type: string;
    value?: string;
    children?: HastNodeLike[];
}

const hastNodeToPlainText = (node: HastNodeLike | undefined): string => {
    if (!node) return '';
    if (node.type === 'text') return node.value ?? '';
    return (node.children ?? []).map(hastNodeToPlainText).join('');
};

const openLinkExternally = (href: string) => {
    openUrl(href).catch(() => window.open(href, '_blank', 'noopener,noreferrer'));
};

const markdownComponents: Components = {
    a({ href, children }: ComponentProps<'a'>) {
        return (
            <a
                href={href}
                onClick={(e) => {
                    if (!href) return;
                    e.preventDefault();
                    openLinkExternally(href);
                }}
            >
                {children}
            </a>
        );
    },
    table({ children }: ComponentProps<'table'>) {
        return (
            <div className="my-2 overflow-x-auto rounded-lg border border-gray-200">
                <table className="!my-0 min-w-full text-[13px]">{children}</table>
            </div>
        );
    },
    th({ children, style }: ComponentProps<'th'>) {
        const align = style?.textAlign;
        return (
            <th className={`!px-3 !py-1.5 font-semibold ${align === 'right' ? 'text-right' : align === 'center' ? 'text-center' : 'text-left'}`}>
                {children}
            </th>
        );
    },
    td({ children, style }: ComponentProps<'td'>) {
        const align = style?.textAlign;
        return (
            <td className={`!px-3 !py-1.5 ${align === 'right' ? 'text-right' : align === 'center' ? 'text-center' : 'text-left'}`}>
                {children}
            </td>
        );
    },
    input({ type, checked }: ComponentProps<'input'>) {
        if (type !== 'checkbox') return null;
        return <input type="checkbox" checked={checked} disabled readOnly className="mr-2 align-middle" />;
    },
    img({ src, alt }: ComponentProps<'img'>) {
        return <img src={src} alt={alt ?? ''} className="max-w-full rounded-lg" loading="lazy" />;
    },
    // Fenced blocks arrive as <pre><code/></pre>; handling them at <pre> covers fences with no
    // language, which a language-class check on <code> alone misses.
    pre({ node, children }: ComponentProps<'pre'> & { node?: HastNodeLike }) {
        const codeElement = node?.children?.[0] as (HastNodeLike & { properties?: { className?: string[] } }) | undefined;
        const languageClass = codeElement?.properties?.className?.find((c) => c.startsWith('language-'));
        const language = languageClass?.replace('language-', '') ?? '';
        const codeText = hastNodeToPlainText(codeElement).replace(/\n$/, '');
        return (
            <div className="group/code my-2 overflow-hidden rounded-lg border border-gray-200 bg-gray-50">
                <div className="flex items-center justify-between px-3 py-1 text-xs text-gray-500">
                    <span className="font-mono">{language}</span>
                    <button
                        onClick={() => navigator.clipboard?.writeText(codeText)}
                        className="opacity-0 transition-opacity hover:text-gray-900 group-hover/code:opacity-100"
                    >
                        Copy
                    </button>
                </div>
                <pre className="!m-0 !border-0 !bg-transparent overflow-x-auto !px-3 !pt-0 !pb-3 text-[13px] leading-5">{children as ReactNode}</pre>
            </div>
        );
    },
    code({ className, children }: ComponentProps<'code'>) {
        const isBlockCode = /\b(hljs|language-)/.test(className ?? '') || String(children).includes('\n');
        if (isBlockCode) {
            return <code className={`${className ?? ''} !bg-transparent !p-0 font-mono`}>{children}</code>;
        }
        return (
            <code className="rounded bg-gray-200/70 px-1.5 py-0.5 font-mono text-[13px] text-gray-900">{children}</code>
        );
    },
};

const markdownProseClasses = [
    'markdown-content prose prose-slate max-w-none break-words text-[15px] leading-6',
    'prose-p:my-1.5 prose-headings:mt-3 prose-headings:mb-1.5 prose-ul:my-1.5 prose-ol:my-1.5 prose-li:my-0',
    'prose-hr:my-3 prose-blockquote:font-normal prose-blockquote:not-italic [&_li>p]:my-0',
    'prose-code:before:content-none prose-code:after:content-none',
].join(' ');

interface MarkdownContentProps {
    content: string;
    isStreaming?: boolean;
    className?: string;
}

/** The one place assistant-visible text is rendered, so markup never reaches the user. */
export const MarkdownContent = memo(function MarkdownContent({ content, isStreaming = false, className }: MarkdownContentProps) {
    const preparedMarkdown = useMemo(() => {
        const withMath = preprocessLaTeX(content);
        return isStreaming ? closeOpenMarkdownConstructs(withMath) : withMath;
    }, [content, isStreaming]);

    return (
        <div className={`${markdownProseClasses} ${className ?? ''}`}>
            <ReactMarkdown
                remarkPlugins={markdownRemarkPlugins}
                rehypePlugins={markdownRehypePlugins}
                components={markdownComponents}
            >
                {preparedMarkdown}
            </ReactMarkdown>
        </div>
    );
});
