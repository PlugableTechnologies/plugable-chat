import type { Page } from '@playwright/test';

/**
 * A stand-in for the Tauri backend, installed before the app's scripts run. The app's own
 * `invoke` and `listen` go through `window.__TAURI_INTERNALS__`, so the real React screens render
 * unchanged and only the Rust side is replaced. Commands the test does not script answer null.
 *
 * What this can show: what a screen does with an answer. What it cannot show: whether the real
 * backend gives that answer; that is the Rust unit tests' job.
 */

export interface McpServerFixture {
    id: string;
    name: string;
    enabled: boolean;
    command: string | null;
    args?: string[];
}

export interface DatabaseSourceFixture {
    id: string;
    name: string;
    kind?: string;
    enabled: boolean;
    command?: string | null;
    project_id?: string;
}

export interface MockOptions {
    mcpServers?: McpServerFixture[];
    databaseToolbox?: { enabled: boolean; sources: DatabaseSourceFixture[] };
    /** Answers by command name; an `{ __error: text }` object rejects instead. */
    responses?: Record<string, unknown>;
    /** `test_mcp_server_config` rejects with this text for the server with this id (the process cannot start). */
    mcpStartFailures?: Record<string, string>;
    /** Tables the table-search command returns once a schema refresh has run. */
    tablesAfterRefresh?: unknown[];
    startup?: 'ready' | 'failed-while-downloading';
}

export interface MockCall {
    cmd: string;
    args: unknown;
}

declare global {
    interface Window {
        __mockEmit: (event: string, payload: unknown) => void;
        __mockCalls: MockCall[];
    }
}

function installMock(opts: MockOptions) {
    const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));
    const settings = {
        system_prompt: 'You are a helpful AI assistant.',
        selected_model: 'qwen3.5-4b',
        mcp_servers: (opts.mcpServers ?? []).map((s) => ({
            transport: { type: 'stdio' },
            args: [],
            env: {},
            auto_approve_tools: false,
            defer_tools: true,
            ...s,
        })),
        chat_format_default: 'openai_completions',
        chat_format_overrides: {},
        tool_call_formats: { enabled: ['native', 'hermes', 'code_mode'], primary: 'native' },
        tool_system_prompts: {},
        tool_search_max_results: 3,
        python_tool_calling_enabled: true,
        legacy_tool_call_format_enabled: false,
        tool_use_examples_enabled: false,
        tool_use_examples_max: 2,
        database_toolbox: opts.databaseToolbox ?? { enabled: false, sources: [] },
        rag_chunk_min_relevancy: 0.3,
        schema_relevancy_threshold: 0.4,
        rag_dominant_threshold: 0.6,
        always_on_builtin_tools: [],
        always_on_mcp_tools: [],
        always_on_tables: [],
        always_on_rag_paths: [],
        first_run_completed: true,
    };
    let tables: unknown[] = [];
    let network = { https_proxy: null, hf_endpoint: null, offline_model_folder: null };
    const failedWhileDownloading = opts.startup === 'failed-while-downloading';
    const ok = (status: string) => ({ status });
    const snapshot = {
        startup_state: failedWhileDownloading ? { state: 'failed', message: 'No compatible model available.' } : { state: 'ready' },
        subsystem_status: {
            foundry_service: ok('ready'),
            model: failedWhileDownloading ? { status: 'failed', message: 'No compatible model available.' } : ok('ready'),
            cpu_embedding: ok('ready'),
            mcp_servers: ok('ready'),
            settings: ok('ready'),
        },
        model_state: failedWhileDownloading
            ? { state: 'error', message: 'Model is still downloading', model_id: 'qwen3.5-4b' }
            : { state: 'ready', model_id: 'qwen3.5-4b' },
        available_models: ['qwen3.5-4b'],
        model_info: [{ id: 'qwen3.5-4b', family: 'qwen', tool_calling: true, tool_format: 'hermes', vision: false, reasoning: false, reasoning_format: 'none', max_input_tokens: 32768, max_output_tokens: 8192, supports_tool_calling: true, supports_temperature: true, supports_top_p: true, supports_reasoning_effort: false }],
        current_model: 'qwen3.5-4b',
        timestamp: 1,
    };

    const calls: MockCall[] = [];
    window.__mockCalls = calls;
    const callbacks = new Map<number, (e: unknown) => void>();
    const listeners = new Map<string, number[]>();
    let nextId = 1;

    const reject = (message: string) => Promise.reject(message);

    const handlers: Record<string, (args: any) => unknown> = {
        frontend_ready: () => snapshot,
        get_launch_overrides: () => ({}),
        get_settings: () => clone(settings),
        get_models: () => ['qwen3.5-4b'],
        get_all_chats: () => [],
        // With nothing cached the app starts its own default-model download, as it does on a first run.
        get_cached_models: () => (failedWhileDownloading ? [] : [{ alias: 'qwen3.5-4b', model_id: 'qwen3.5-4b' }]),
        download_model: () => new Promise(() => {}),
        get_catalog_models: () => [],
        get_loaded_models: () => [],
        get_current_model: () => snapshot.model_info[0],
        get_model_info: () => snapshot.model_info,
        get_model_state: () => ({ state: snapshot.model_state, timestamp: snapshot.timestamp }),
        get_cached_database_schemas: () => [],
        get_pending_tool_approvals: () => [],
        get_python_allowed_imports: () => ['math', 'json'],
        get_version_info: () => ({ version: '0.0.0-test' }),
        sync_mcp_servers: () => [],
        list_mcp_tools: () => [],
        test_mcp_server_config: ({ config }) => {
            const failure = opts.mcpStartFailures?.[config.id];
            return failure ? reject(failure) : [];
        },
        get_toolbox_install_status: () => ({ toolbox_path: '/usr/bin/toolbox', download_supported: true, download_size_bytes: null, pinned_version: '1.0.0' }),
        get_network_settings: () => clone(network),
        save_network_settings: ({ network: n }) => {
            network = clone(n);
            return clone(network);
        },
        // rc18's backend saved the server and only logged a failed start, so this accepts the save.
        update_mcp_server: ({ config }) => {
            settings.mcp_servers = settings.mcp_servers.map((s) => (s.id === config.id ? config : s));
            return null;
        },
        update_database_toolbox_config: ({ config }) => {
            settings.database_toolbox = clone(config);
            return null;
        },
        refresh_database_schema_for_source: () => {
            tables = clone(opts.tablesAfterRefresh ?? []);
            return { sources: [], errors: [] };
        },
        refresh_database_schemas: () => {
            tables = clone(opts.tablesAfterRefresh ?? []);
            return { sources: [], errors: [] };
        },
        search_database_tables: () => clone(tables),
        'plugin:event|listen': ({ event, handler }) => {
            listeners.set(event, [...(listeners.get(event) ?? []), handler]);
            return handler;
        },
        'plugin:event|unlisten': () => null,
    };

    const internals = {
        invoke: async (cmd: string, args: unknown) => {
            calls.push({ cmd, args: clone(args ?? null) });
            const scripted = opts.responses?.[cmd];
            if (scripted && typeof scripted === 'object' && '__error' in scripted) return reject(String((scripted as { __error: unknown }).__error));
            if (scripted !== undefined) return clone(scripted);
            const handler = handlers[cmd];
            return handler ? handler(args) : null;
        },
        transformCallback: (cb: (e: unknown) => void) => {
            const id = nextId++;
            callbacks.set(id, cb);
            return id;
        },
        unregisterCallback: (id: number) => callbacks.delete(id),
        convertFileSrc: (p: string) => p,
        metadata: { currentWindow: { label: 'main' }, currentWebview: { windowLabel: 'main', label: 'main' } },
    };
    Object.assign(window, {
        __TAURI_INTERNALS__: internals,
        __TAURI_EVENT_PLUGIN_INTERNALS__: { unregisterListener: () => {} },
        __mockEmit: (event: string, payload: unknown) => {
            for (const id of listeners.get(event) ?? []) callbacks.get(id)?.({ event, id, payload });
        },
    });
}

/** Installs the mock and loads the app; resolves once the first screen is up. */
export async function openApp(page: Page, opts: MockOptions = {}) {
    await page.addInitScript(installMock, opts);
    await page.goto('/');
    await page.locator('#root').waitFor();
}

export const emit = (page: Page, event: string, payload: unknown) => page.evaluate(([e, p]) => window.__mockEmit(e as string, p), [event, payload] as const);

export const callsTo = async (page: Page, cmd: string): Promise<MockCall[]> =>
    (await page.evaluate(() => window.__mockCalls)).filter((c) => c.cmd === cmd);
