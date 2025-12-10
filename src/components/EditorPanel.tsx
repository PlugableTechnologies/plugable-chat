import Editor from '@monaco-editor/react';
import { X, Copy, Check } from 'lucide-react';
import { useChatStore } from '../store/chat-store';
import { useState } from 'react';

export function EditorPanel() {
    const { editorContent, editorLanguage, isEditorOpen, setEditorOpen } = useChatStore();
    const [copied, setCopied] = useState(false);

    if (!isEditorOpen) return null;

    const handleCopy = async () => {
        await navigator.clipboard.writeText(editorContent);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
    };

    return (
        <div id="editor-panel" className="editor-panel w-[500px] h-full bg-[#0d1117] border-l border-gray-700 flex flex-col shadow-xl animate-in slide-in-from-right-10 duration-200">
            {/* Header */}
            <div className="editor-header flex items-center justify-between px-4 py-3 border-b border-gray-700 bg-[#161b22]">
                <div className="editor-title-row flex items-center gap-2">
                    <span className="editor-title text-sm font-bold text-white">Code Editor</span>
                    <span className="editor-language-pill text-xs text-gray-400 px-2 py-0.5 bg-gray-800 rounded-full border border-gray-700 uppercase">
                        {editorLanguage}
                    </span>
                </div>
                <div className="editor-actions flex items-center gap-1">
                    <button
                        onClick={handleCopy}
                        className="p-1.5 text-gray-400 hover:text-white hover:bg-gray-700 rounded-lg transition-colors"
                        title="Copy code"
                    >
                        {copied ? <Check size={16} className="text-green-400" /> : <Copy size={16} />}
                    </button>
                    <button
                        onClick={() => setEditorOpen(false)}
                        className="p-1.5 text-gray-400 hover:text-white hover:bg-gray-700 rounded-lg transition-colors"
                        title="Close editor"
                    >
                        <X size={18} />
                    </button>
                </div>
            </div>

            {/* Editor */}
            <div className="flex-1 overflow-hidden">
                <Editor
                    height="100%"
                    defaultLanguage="typescript"
                    language={editorLanguage}
                    value={editorContent}
                    theme="vs-dark"
                    options={{
                        minimap: { enabled: false },
                        fontSize: 14,
                        fontFamily: "'JetBrains Mono', 'Fira Code', Consolas, monospace",
                        scrollBeyondLastLine: false,
                        padding: { top: 16, bottom: 16 },
                        lineNumbers: 'on',
                        renderLineHighlight: 'all',
                        automaticLayout: true,
                    }}
                />
            </div>
        </div>
    );
}
