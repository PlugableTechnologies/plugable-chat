import Editor from '@monaco-editor/react';
import { useChatStore } from '../store/chat-store';
import { X } from 'lucide-react';

export function CodeEditor() {
  const { toggleCodeEditor } = useChatStore();

  return (
    <div className="h-full w-full bg-white border-l border-gray-200 flex flex-col">
        <div className="bg-gray-100 text-gray-900 px-4 py-2 text-sm font-medium flex justify-between items-center shrink-0">
            <span>Code Editor</span>
            <div className="flex gap-2">
                <button className="text-xs bg-gray-200 px-2 py-1 rounded hover:bg-gray-300">Copy</button>
                <button onClick={toggleCodeEditor} className="hover:text-gray-600">
                    <X size={16} />
                </button>
            </div>
        </div>
        <div className="flex-1 overflow-hidden">
            <Editor 
                height="100%" 
                defaultLanguage="rust" 
                defaultValue="// Write your code here" 
                theme="vs"
                options={{
                    minimap: { enabled: false },
                    fontSize: 14,
                    scrollBeyondLastLine: false,
                    automaticLayout: true,
                }}
            />
        </div>
    </div>
  );
}

