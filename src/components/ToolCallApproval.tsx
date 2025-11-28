import { useState } from 'react';
import { Check, X, Loader2, AlertTriangle, Terminal } from 'lucide-react';
import type { ToolCallState } from '../lib/tool-calls';

interface ToolCallApprovalProps {
    toolCall: ToolCallState;
    autoApprove: boolean;
    onApprove: () => void;
    onReject: () => void;
}

export function ToolCallApproval({ toolCall, autoApprove, onApprove, onReject }: ToolCallApprovalProps) {
    const [expanded, setExpanded] = useState(false);
    
    // Status-specific rendering
    const statusIcon = {
        pending: <Terminal size={16} className="text-blue-500" />,
        approved: <Check size={16} className="text-green-500" />,
        rejected: <X size={16} className="text-red-500" />,
        executing: <Loader2 size={16} className="text-blue-500 animate-spin" />,
        completed: <Check size={16} className="text-green-500" />,
        error: <AlertTriangle size={16} className="text-red-500" />,
    };
    
    const statusLabel = {
        pending: autoApprove ? 'Auto-executing...' : 'Awaiting approval',
        approved: 'Approved',
        rejected: 'Rejected',
        executing: 'Executing...',
        completed: 'Completed',
        error: 'Failed',
    };
    
    const statusBg = {
        pending: 'bg-blue-50 border-blue-200',
        approved: 'bg-green-50 border-green-200',
        rejected: 'bg-red-50 border-red-200',
        executing: 'bg-blue-50 border-blue-200',
        completed: 'bg-green-50 border-green-200',
        error: 'bg-red-50 border-red-200',
    };
    
    return (
        <div className={`rounded-xl border p-4 my-3 ${statusBg[toolCall.status]}`}>
            {/* Header */}
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                    {statusIcon[toolCall.status]}
                    <span className="font-medium text-gray-900">Tool Call</span>
                    <span className="text-xs text-gray-500">{statusLabel[toolCall.status]}</span>
                </div>
                
                {toolCall.status === 'pending' && !autoApprove && (
                    <div className="flex gap-2">
                        <button
                            onClick={onApprove}
                            className="flex items-center gap-1 px-3 py-1.5 bg-green-600 text-white text-xs font-medium rounded-lg hover:bg-green-700"
                        >
                            <Check size={14} />
                            Approve
                        </button>
                        <button
                            onClick={onReject}
                            className="flex items-center gap-1 px-3 py-1.5 bg-gray-200 text-gray-700 text-xs font-medium rounded-lg hover:bg-gray-300"
                        >
                            <X size={14} />
                            Reject
                        </button>
                    </div>
                )}
            </div>
            
            {/* Tool info */}
            <div className="mt-3 space-y-2">
                <div className="flex gap-4 text-sm">
                    <div>
                        <span className="text-gray-500">Server:</span>{' '}
                        <span className="font-mono text-gray-900">{toolCall.server}</span>
                    </div>
                    <div>
                        <span className="text-gray-500">Tool:</span>{' '}
                        <span className="font-mono text-gray-900">{toolCall.tool}</span>
                    </div>
                </div>
                
                {/* Arguments (collapsible) */}
                <div>
                    <button
                        onClick={() => setExpanded(!expanded)}
                        className="text-xs text-gray-500 hover:text-gray-700"
                    >
                        {expanded ? '▼ Hide arguments' : '▶ Show arguments'}
                    </button>
                    {expanded && (
                        <pre className="mt-2 p-2 bg-white rounded-lg text-xs font-mono overflow-x-auto border border-gray-200">
                            {JSON.stringify(toolCall.arguments, null, 2)}
                        </pre>
                    )}
                </div>
            </div>
            
            {/* Result */}
            {toolCall.result && (
                <div className="mt-3 p-3 bg-white rounded-lg border border-gray-200">
                    <div className="text-xs font-medium text-gray-500 mb-1">Result:</div>
                    <pre className="text-sm font-mono whitespace-pre-wrap text-gray-900">
                        {toolCall.result}
                    </pre>
                </div>
            )}
            
            {/* Error */}
            {toolCall.error && (
                <div className="mt-3 p-3 bg-red-100 rounded-lg border border-red-200">
                    <div className="text-xs font-medium text-red-600 mb-1">Error:</div>
                    <pre className="text-sm font-mono whitespace-pre-wrap text-red-800">
                        {toolCall.error}
                    </pre>
                </div>
            )}
        </div>
    );
}

// Container for multiple tool calls
interface ToolCallsContainerProps {
    toolCalls: ToolCallState[];
    onApprove: (id: string) => void;
    onReject: (id: string) => void;
    autoApproveMap: Record<string, boolean>; // server_id -> auto_approve
}

export function ToolCallsContainer({ toolCalls, onApprove, onReject, autoApproveMap }: ToolCallsContainerProps) {
    if (toolCalls.length === 0) return null;
    
    return (
        <div className="space-y-2">
            {toolCalls.map((call) => (
                <ToolCallApproval
                    key={call.id}
                    toolCall={call}
                    autoApprove={autoApproveMap[call.server] || false}
                    onApprove={() => onApprove(call.id)}
                    onReject={() => onReject(call.id)}
                />
            ))}
        </div>
    );
}


