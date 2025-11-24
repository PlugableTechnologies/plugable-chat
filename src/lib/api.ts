import { invoke as tauriInvoke } from '@tauri-apps/api/core';

const isTauri = () => {
    return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
};

export async function invoke<T>(cmd: string, args?: any): Promise<T> {
    if (isTauri()) {
        try {
            return await tauriInvoke(cmd, args);
        } catch (e) {
            console.error(`Tauri invoke failed for ${cmd}:`, e);
            throw e;
        }
    } else {
        console.log(`[Mock] Invoking ${cmd} with args:`, args);
        return mockInvoke(cmd, args);
    }
}

async function mockInvoke(cmd: string, _args?: any): Promise<any> {
    console.warn(`[Tauri Bridge] '${cmd}' called but Tauri is not detected. Ensure you are running in the Tauri window for real backend functionality.`);

    // Return safe empty defaults to prevent UI crashes in browser view
    switch (cmd) {
        case 'get_models': return [];
        case 'get_all_chats': return [];
        default: return null;
    }
}

import { listen as tauriListen, Event } from '@tauri-apps/api/event';

export async function listen<T>(event: string, handler: (event: Event<T>) => void): Promise<() => void> {
    if (isTauri()) {
        return await tauriListen(event, handler);
    } else {
        console.log(`[Mock] Listening for ${event}`);
        // Return a dummy unlisten function
        return () => console.log(`[Mock] Unlistened to ${event}`);
    }
}
