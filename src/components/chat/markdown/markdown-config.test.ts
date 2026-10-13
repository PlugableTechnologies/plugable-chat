import assert from 'node:assert/strict';
import { test } from 'node:test';
import { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ReactMarkdown from 'react-markdown';
import { markdownRehypePlugins, markdownRemarkPlugins } from './markdown-config.ts';

const renderMarkdownToHtml = (markdown: string): string =>
    renderToStaticMarkup(
        createElement(ReactMarkdown, { remarkPlugins: markdownRemarkPlugins, rehypePlugins: markdownRehypePlugins }, markdown)
    );

const visibleTextOf = (html: string): string => html.replace(/<[^>]*>/g, '');

const kitchenSink = [
    '# Heading',
    '**bold** *italic* ~~gone~~ `inline`',
    '- one\n  - nested',
    '- [x] done task',
    '| Month | Sales |\n|---|--:|\n| 2025-01 | $204,678.04 |\n| a<br>b | 2 |',
    '```sql\nSELECT 1\n```',
    '```\nno language\n```',
    '> quote',
    '[link](https://plugable.com)',
    '---',
    'Inline math $x^2$ and\n\n$$\\boxed{42}$$',
].join('\n\n');

test('renders markdown constructs without leaking markup characters', () => {
    const text = visibleTextOf(renderMarkdownToHtml(kitchenSink));
    for (const leaked of ['**', '|---', '## ', '~~', '[x]', '](', '```']) {
        assert.ok(!text.includes(leaked), `leaked ${JSON.stringify(leaked)} in: ${text}`);
    }
});

test('renders structural elements', () => {
    const html = renderMarkdownToHtml(kitchenSink);
    for (const tag of ['<h1', '<strong', '<em', '<del', '<table', '<ul', '<blockquote', '<hr', '<a ', 'katex']) {
        assert.ok(html.includes(tag), `missing ${tag}`);
    }
    assert.ok(html.includes('hljs'), 'fenced code is highlighted');
});

test('inline <br> in a table cell becomes a line break, not visible text', () => {
    const text = visibleTextOf(renderMarkdownToHtml('| a |\n|---|\n| x<br>y |'));
    assert.ok(!text.includes('<br'));
});

test('script tags are stripped', () => {
    const html = renderMarkdownToHtml('hi <script>alert(1)</script> there');
    assert.ok(!html.includes('<script'));
});
