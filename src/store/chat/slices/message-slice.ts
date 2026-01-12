import type { StateCreator } from 'zustand';
import type { Message } from '../types';

export interface MessageSlice {
    chatMessages: Message[];
    appendChatMessage: (msg: Message) => void;
    chatInputValue: string;
    setChatInputValue: (s: string) => void;
    chatGenerationCounter: number;
}

export const createMessageSlice: StateCreator<
    MessageSlice,
    [],
    [],
    MessageSlice
> = (set) => ({
    chatMessages: [],
    appendChatMessage: (msg) =>
        set((state) => ({ chatMessages: [...state.chatMessages, msg] })),
    chatInputValue: '',
    setChatInputValue: (chatInputValue) => set({ chatInputValue }),
    chatGenerationCounter: 0,
});
