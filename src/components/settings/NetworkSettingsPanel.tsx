import { useCallback, useEffect, useState } from 'react';
import { CheckCircle2, FolderOpen, Loader2, RefreshCw, Save, Wifi, XCircle } from 'lucide-react';
import { invoke } from '../../lib/api';
import {
    EMPTY_NETWORK_FORM,
    loadNetworkForm,
    networkFormIsDirty,
    saveNetworkForm,
    summarizeHostRows,
    testNetworkForm,
    validateNetworkForm,
    type HostCheckRow,
    type NetworkForm,
} from '../../lib/network-settings';

/**
 * Settings > Network: a proxy, a Hugging Face mirror and a folder holding the search model, for
 * computers where the default downloads are blocked. Self-contained (own Save button) so it does
 * not touch the shared tab plumbing.
 */
export function NetworkSettingsPanel() {
    const [saved, setSaved] = useState<NetworkForm>(EMPTY_NETWORK_FORM);
    const [form, setForm] = useState<NetworkForm>(EMPTY_NETWORK_FORM);
    const [loading, setLoading] = useState(true);
    const [saving, setSaving] = useState(false);
    const [message, setMessage] = useState<{ tone: 'ok' | 'error'; text: string } | null>(null);
    const [testing, setTesting] = useState(false);
    const [rows, setRows] = useState<HostCheckRow[] | null>(null);
    const [retrying, setRetrying] = useState(false);

    useEffect(() => {
        loadNetworkForm(invoke)
            .then((f) => {
                setSaved(f);
                setForm(f);
            })
            .catch((e) => setMessage({ tone: 'error', text: `Could not read the saved settings: ${String(e)}` }))
            .finally(() => setLoading(false));
    }, []);

    const errors = validateNetworkForm(form);
    const dirty = networkFormIsDirty(saved, form);
    const canSave = dirty && !saving && !errors.httpsProxy && !errors.hfEndpoint;

    const update = (patch: Partial<NetworkForm>) => {
        setForm((f) => ({ ...f, ...patch }));
        setMessage(null);
    };

    const onSaveNetworkSettings = useCallback(async () => {
        setSaving(true);
        setMessage(null);
        try {
            const next = await saveNetworkForm(form, invoke);
            setSaved(next);
            setForm(next);
            setMessage({ tone: 'ok', text: 'Saved. The proxy and mirror apply the next time Plugable Chat starts; the model folder is used when the search model next loads.' });
        } catch (e) {
            setMessage({ tone: 'error', text: e instanceof Error ? e.message : String(e) });
        } finally {
            setSaving(false);
        }
    }, [form]);

    const onTestNetworkConnection = async () => {
        setTesting(true);
        setRows(null);
        try {
            setRows(await testNetworkForm(form, invoke));
        } catch (e) {
            setMessage({ tone: 'error', text: e instanceof Error ? e.message : String(e) });
        } finally {
            setTesting(false);
        }
    };

    const onChooseModelFolder = async () => {
        try {
            const path = await invoke<string | null>('select_folder');
            if (path) update({ offlineModelFolder: path });
        } catch (e) {
            console.error('[NetworkSettingsPanel] select_folder failed', e);
        }
    };

    const onRetrySearchModel = async () => {
        setRetrying(true);
        try {
            await invoke('retry_embedding_init');
            setMessage({ tone: 'ok', text: 'The search model loaded.' });
        } catch (e) {
            setMessage({ tone: 'error', text: String(e) });
        } finally {
            setRetrying(false);
        }
    };

    if (loading) {
        return <div className="flex items-center justify-center py-12"><Loader2 className="animate-spin text-blue-500" size={20} /></div>;
    }

    const summary = rows ? summarizeHostRows(rows) : null;

    return (
        <div id="network-settings" className="network-settings space-y-6">
            <p className="text-xs text-gray-500">
                Only needed when downloads are blocked on this network. Leave a field empty to use the defaults.
                The search model comes from the Plugable Chat release on GitHub first, then from Hugging Face.
            </p>

            <label className="block">
                <span className="text-sm font-medium text-gray-700">Proxy address</span>
                <input
                    id="network-https-proxy"
                    type="text"
                    value={form.httpsProxy}
                    onChange={(e) => update({ httpsProxy: e.target.value })}
                    placeholder="http://proxy.example.com:8080"
                    className="mt-1 w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                {errors.httpsProxy && <span className="text-xs text-red-600">{errors.httpsProxy}</span>}
                <span className="block text-xs text-gray-400 mt-1">Used for downloads. Takes effect the next time the app starts.</span>
            </label>

            <label className="block">
                <span className="text-sm font-medium text-gray-700">Hugging Face mirror</span>
                <input
                    id="network-hf-endpoint"
                    type="text"
                    value={form.hfEndpoint}
                    onChange={(e) => update({ hfEndpoint: e.target.value })}
                    placeholder="https://hf-mirror.example.com"
                    className="mt-1 w-full px-3 py-2 text-sm border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
                />
                {errors.hfEndpoint && <span className="text-xs text-red-600">{errors.hfEndpoint}</span>}
                <span className="block text-xs text-gray-400 mt-1">Replaces huggingface.co for the search model if the GitHub copy is unavailable. Takes effect the next time the app starts.</span>
            </label>

            <div>
                <span className="text-sm font-medium text-gray-700">Search model folder</span>
                <div className="mt-1 flex gap-2">
                    <input
                        id="network-offline-model-folder"
                        type="text"
                        value={form.offlineModelFolder}
                        onChange={(e) => update({ offlineModelFolder: e.target.value })}
                        placeholder="Folder containing model.onnx, tokenizer.json, config.json, ..."
                        className="flex-1 px-3 py-2 text-sm border border-gray-200 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
                    />
                    <button type="button" onClick={onChooseModelFolder} className="flex items-center gap-1.5 px-3 py-2 text-sm rounded-lg border border-gray-200 text-gray-700 hover:bg-gray-50">
                        <FolderOpen size={16} /> Choose
                    </button>
                </div>
                <span className="block text-xs text-gray-400 mt-1">
                    Files of bge-base-en-v1.5 (revision 4d6cd88) that you copied from another computer. Each file is checked against its published checksum before it is used.
                </span>
            </div>

            <div className="flex flex-wrap items-center gap-2">
                <button
                    type="button"
                    onClick={onSaveNetworkSettings}
                    disabled={!canSave}
                    className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg bg-blue-600 text-white hover:bg-blue-700 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                    {saving ? <Loader2 size={16} className="animate-spin" /> : <Save size={16} />} Save
                </button>
                <button
                    type="button"
                    onClick={onTestNetworkConnection}
                    disabled={testing || !!errors.httpsProxy || !!errors.hfEndpoint}
                    className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg border border-gray-200 text-gray-700 hover:bg-gray-50 disabled:opacity-50"
                >
                    {testing ? <Loader2 size={16} className="animate-spin" /> : <Wifi size={16} />} Test connection
                </button>
                <button
                    type="button"
                    onClick={onRetrySearchModel}
                    disabled={retrying || dirty}
                    title={dirty ? 'Save first' : 'Load the search model again now'}
                    className="flex items-center gap-1.5 px-3 py-1.5 text-sm font-medium rounded-lg border border-gray-200 text-gray-700 hover:bg-gray-50 disabled:opacity-50"
                >
                    {retrying ? <Loader2 size={16} className="animate-spin" /> : <RefreshCw size={16} />} Retry search model
                </button>
            </div>

            {message && (
                <p className={`text-sm ${message.tone === 'ok' ? 'text-green-700' : 'text-red-600'}`} role="status">{message.text}</p>
            )}

            {rows && summary && (
                <div id="network-test-results" className="rounded-lg border border-gray-200 p-3">
                    <p className="text-sm font-medium text-gray-700 mb-2">
                        {summary.allOk ? 'Every address answered.' : `${summary.failed} of ${rows.length} addresses did not answer.`}
                    </p>
                    <ul className="space-y-1">
                        {rows.map((row, i) => (
                            <li key={row.host} className="flex items-start gap-2 text-xs text-gray-700">
                                {row.ok ? <CheckCircle2 size={14} className="text-green-600 mt-0.5 shrink-0" /> : <XCircle size={14} className="text-red-600 mt-0.5 shrink-0" />}
                                <span className="break-all">{summary.lines[i]}</span>
                            </li>
                        ))}
                    </ul>
                </div>
            )}
        </div>
    );
}
