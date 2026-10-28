import type { StateCreator } from 'zustand';
import { invoke } from '../../../lib/api';
import { activeDirectTool, type DirectTool, type DirectToolRef } from '../../../lib/direct-tools';

export interface DirectToolSlice {
    /** Tools of connected MCP servers that take one text field (offered in the model drop-down). */
    directTools: DirectTool[];
    /** The tool chosen in the drop-down, remembered between launches; null while a model is chosen. */
    chosenDirectTool: DirectToolRef | null;
    /** `chosenDirectTool`, but only while its server is connected and still offers it. */
    getActiveDirectTool: () => DirectTool | null;
    fetchDirectTools: () => Promise<void>;
    chooseDirectTool: (tool: DirectToolRef | null) => Promise<void>;
}

let rememberedChoiceLoaded = false;

export const createDirectToolSlice: StateCreator<DirectToolSlice, [], [], DirectToolSlice> = (set, get) => ({
    directTools: [],
    chosenDirectTool: null,
    getActiveDirectTool: () => activeDirectTool(get().directTools, get().chosenDirectTool),

    fetchDirectTools: async () => {
        try {
            if (!rememberedChoiceLoaded) {
                const remembered = await invoke<DirectToolRef | null>('get_selected_direct_tool');
                rememberedChoiceLoaded = true;
                if (remembered && !get().chosenDirectTool) set({ chosenDirectTool: remembered });
            }
            const tools = await invoke<DirectTool[] | null>('list_direct_mcp_tools');
            set({ directTools: tools ?? [] });
        } catch (e) {
            console.error('[ChatStore] Failed to list the tools offered in the model drop-down:', e);
        }
    },

    chooseDirectTool: async (tool) => {
        set({ chosenDirectTool: tool });
        try {
            await invoke('set_selected_direct_tool', { tool });
        } catch (e) {
            console.error('[ChatStore] Failed to remember the drop-down choice:', e);
        }
    },
});
