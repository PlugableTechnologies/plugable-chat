import { useState, useEffect, useCallback } from 'react';
import { GitBranch, ChevronUp, ChevronDown, Loader2 } from 'lucide-react';
import { invoke } from '../../../lib/api';
import type { StatePreview } from '../types';

// State Machine Preview Component - debug viewer for available states
export function StateMachinePreview() {
    const [states, setStates] = useState<StatePreview[]>([]);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [expanded, setExpanded] = useState(false);

    const fetchStates = useCallback(() => {
        setLoading(true);
        setError(null);
        invoke<StatePreview[]>('get_state_machine_preview')
            .then((data) => {
                setStates(data);
            })
            .catch((e) => {
                console.error('Failed to get state machine preview:', e);
                setError(e.message || String(e));
            })
            .finally(() => setLoading(false));
    }, []);

    useEffect(() => {
        fetchStates();
    }, [fetchStates]);

    return (
        <div className="border border-gray-200 rounded-xl overflow-hidden">
            <button
                onClick={() => setExpanded(!expanded)}
                className="w-full flex items-center justify-between px-4 py-3 bg-gray-50 hover:bg-gray-100 text-sm font-medium text-gray-700"
            >
                <span className="flex items-center gap-2">
                    <GitBranch size={16} />
                    State Machine Preview
                    <span className="text-xs bg-purple-100 text-purple-700 px-2 py-0.5 rounded-full">
                        {states.length} states
                    </span>
                </span>
                {expanded ? <ChevronUp size={16} /> : <ChevronDown size={16} />}
            </button>

            {expanded && (
                <div className="p-4 bg-white border-t border-gray-200 space-y-3">
                    {loading ? (
                        <div className="flex items-center gap-2 text-sm text-gray-600">
                            <Loader2 size={16} className="animate-spin" />
                            Loading states...
                        </div>
                    ) : error ? (
                        <div className="text-sm text-red-700 bg-red-50 px-3 py-2 rounded-lg">{error}</div>
                    ) : states.length > 0 ? (
                        <div className="space-y-2">
                            {states.map((state, idx) => (
                                <div
                                    key={idx}
                                    className={`p-3 rounded-lg border ${state.is_possible ? 'border-green-200 bg-green-50/50' : 'border-gray-200 bg-gray-50'}`}
                                >
                                    <div className="flex items-center justify-between mb-1">
                                        <span className="font-medium text-sm text-gray-900">{state.name}</span>
                                        {state.is_possible && (
                                            <span className="text-xs bg-green-100 text-green-700 px-2 py-0.5 rounded-full">
                                                possible
                                            </span>
                                        )}
                                    </div>
                                    <p className="text-xs text-gray-600 mb-2">{state.description}</p>
                                    {state.available_tools.length > 0 && (
                                        <div className="flex flex-wrap gap-1">
                                            {state.available_tools.map((tool, tidx) => (
                                                <span
                                                    key={tidx}
                                                    className="text-xs bg-blue-100 text-blue-700 px-2 py-0.5 rounded"
                                                >
                                                    {tool}
                                                </span>
                                            ))}
                                        </div>
                                    )}
                                </div>
                            ))}
                        </div>
                    ) : (
                        <p className="text-sm text-gray-500">No states available.</p>
                    )}
                    <button
                        onClick={fetchStates}
                        className="text-xs text-blue-600 hover:text-blue-700"
                    >
                        Refresh
                    </button>
                </div>
            )}
        </div>
    );
}
