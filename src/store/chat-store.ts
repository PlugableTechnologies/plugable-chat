import { create } from 'zustand'

export interface Message {
  id: string;
  role: 'user' | 'assistant';
  content: string;
  timestamp: number;
}

interface ChatState {
  messages: Message[];
  addMessage: (msg: Message) => void;
  input: string;
  setInput: (s: string) => void;
  isLoading: boolean;
  setIsLoading: (loading: boolean) => void;
  isCodeEditorOpen: boolean;
  toggleCodeEditor: () => void;
}

export const useChatStore = create<ChatState>((set) => ({
  messages: [],
  addMessage: (msg) => set((state) => ({ messages: [...state.messages, msg] })),
  input: '',
  setInput: (input) => set({ input }),
  isLoading: false,
  setIsLoading: (isLoading) => set({ isLoading }),
  isCodeEditorOpen: false,
  toggleCodeEditor: () => set((state) => ({ isCodeEditorOpen: !state.isCodeEditorOpen })),
}))
