import { useState, useCallback } from 'react';

interface SettingsTagInputProps {
    tags: string[];
    onChange: (tags: string[]) => void;
    placeholder?: string;
}

// Tag input component for args - auto-splits on spaces
export function SettingsTagInput({
    tags,
    onChange,
    placeholder
}: SettingsTagInputProps) {
    const [input, setInput] = useState('');

    const addTags = useCallback(() => {
        const trimmed = input.trim();
        if (!trimmed) {
            setInput('');
            return;
        }

        // Split on spaces, but preserve quoted strings
        const parts: string[] = [];
        let current = '';
        let inQuote = false;
        let quoteChar = '';

        for (let i = 0; i < trimmed.length; i++) {
            const char = trimmed[i];

            if ((char === '"' || char === "'") && !inQuote) {
                inQuote = true;
                quoteChar = char;
            } else if (char === quoteChar && inQuote) {
                inQuote = false;
                quoteChar = '';
            } else if (char === ' ' && !inQuote) {
                if (current) {
                    parts.push(current);
                    current = '';
                }
            } else {
                current += char;
            }
        }
        if (current) {
            parts.push(current);
        }

        // Filter out duplicates and empty strings
        const newParts = parts.filter(p => p && !tags.includes(p));
        if (newParts.length > 0) {
            onChange([...tags, ...newParts]);
        }
        setInput('');
    }, [input, tags, onChange]);

    const handleKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === 'Enter' || e.key === ',') {
            e.preventDefault();
            addTags();
        } else if (e.key === 'Backspace' && !input && tags.length > 0) {
            const newTags = tags.slice(0, -1);
            onChange(newTags);
        }
    };

    const handlePaste = (e: React.ClipboardEvent) => {
        e.preventDefault();
        const pasted = e.clipboardData.getData('text');
        // Set input and immediately trigger split
        setInput(prev => prev + pasted);
        // Use setTimeout to process after state update
        setTimeout(() => {
            const trimmed = (input + pasted).trim();
            if (!trimmed) return;

            // Split on spaces, but preserve quoted strings
            const parts: string[] = [];
            let current = '';
            let inQuote = false;
            let quoteChar = '';

            for (let i = 0; i < trimmed.length; i++) {
                const char = trimmed[i];

                if ((char === '"' || char === "'") && !inQuote) {
                    inQuote = true;
                    quoteChar = char;
                } else if (char === quoteChar && inQuote) {
                    inQuote = false;
                    quoteChar = '';
                } else if (char === ' ' && !inQuote) {
                    if (current) {
                        parts.push(current);
                        current = '';
                    }
                } else {
                    current += char;
                }
            }
            if (current) {
                parts.push(current);
            }

            // Filter out duplicates and empty strings
            const newParts = parts.filter(p => p && !tags.includes(p));
            if (newParts.length > 0) {
                onChange([...tags, ...newParts]);
            }
            setInput('');
        }, 0);
    };

    const removeTag = (index: number) => {
        const newTags = tags.filter((_, i) => i !== index);
        onChange(newTags);
    };

    return (
        <div className="space-y-1">
            <div className="flex flex-wrap gap-1.5 p-2 bg-white border border-gray-200 rounded-lg min-h-[40px] focus-within:border-blue-400 focus-within:ring-1 focus-within:ring-blue-400">
                {tags.map((tag, i) => (
                    <span
                        key={i}
                        className="inline-flex items-center gap-1 px-2 py-0.5 bg-blue-100 text-blue-800 text-xs rounded-md font-mono"
                    >
                        {tag}
                        <button
                            onClick={() => removeTag(i)}
                            className="hover:text-blue-600"
                        >
                            ×
                        </button>
                    </span>
                ))}
                <input
                    type="text"
                    value={input}
                    onChange={(e) => setInput(e.target.value)}
                    onKeyDown={handleKeyDown}
                    onPaste={handlePaste}
                    onBlur={addTags}
                    placeholder={tags.length === 0 ? placeholder : ''}
                    className="flex-1 min-w-[100px] outline-none text-sm bg-transparent font-mono"
                />
            </div>
            <p className="text-[10px] text-gray-400">
                Paste or type multiple args separated by spaces. Use quotes for args with spaces.
            </p>
        </div>
    );
}
