import type { StateCreator } from 'zustand';

export interface EditorSlice {
    isEditorOpen: boolean;
    editorContent: string;
    editorLanguage: string;
    setEditorOpen: (open: boolean) => void;
    setEditorContent: (content: string, language: string) => void;
}

export const createEditorSlice: StateCreator<
    EditorSlice,
    [],
    [],
    EditorSlice
> = (set) => ({
    isEditorOpen: false,
    editorContent: '',
    editorLanguage: 'text',
    setEditorOpen: (open) => set({ isEditorOpen: open }),
    setEditorContent: (content, language) => set({ 
        editorContent: content, 
        editorLanguage: language, 
        isEditorOpen: true 
    }),
});
