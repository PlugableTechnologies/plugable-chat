import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { FALLBACK_PYTHON_ALLOWED_IMPORTS } from './python-allowed-imports';

// ============ Foundry Model Types ============

/** A model from the Foundry catalog (/foundry/list) */
export interface FoundryCatalogModel {
    name: string;
    displayName: string;
    alias: string;
    uri: string;
    version: string;
    fileSizeMb: number;
    license: string;
    task: string;
    supportsToolCalling: boolean;
    runtime: FoundryCatalogModelRuntime;
    publisher: string;
}

/** Runtime info for a catalog model */
export interface FoundryCatalogModelRuntime {
    deviceType: 'CPU' | 'GPU' | 'NPU' | string;
    executionProvider: string;
}

/** Foundry service status from /openai/status */
export interface FoundryServiceStatus {
    endpoints: string[];
    modelDirPath: string;
    isAutoRegistrationResolved: boolean;
}

// ============ API Helpers ============

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
        case 'get_python_allowed_imports': return FALLBACK_PYTHON_ALLOWED_IMPORTS;
        case 'get_catalog_models': return [];
        case 'get_loaded_models': return [];
        case 'get_cached_models': return [];
        case 'get_foundry_service_status': return { endpoints: [], modelDirPath: '', isAutoRegistrationResolved: false };
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
