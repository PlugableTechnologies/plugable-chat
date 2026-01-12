interface ToolApprovalDialogProps {
    calls: { server: string; tool: string; arguments: Record<string, unknown> }[];
    onApprove: () => void;
    onReject: () => void;
}

/**
 * Tool approval dialog component
 * Shows pending tool calls and allows approval/rejection
 */
export const ToolApprovalDialog = ({
    calls,
    onApprove,
    onReject
}: ToolApprovalDialogProps) => {
    return (
        <div className="bg-amber-50 border border-amber-200 rounded-xl p-4 my-4">
            <div className="flex items-start gap-3">
                <span className="text-xl">⚠️</span>
                <div className="flex-1">
                    <h4 className="font-semibold text-amber-900 mb-2">Tool Execution Requires Approval</h4>
                    <div className="space-y-2 mb-4">
                        {calls.map((call, idx) => (
                            <div key={idx} className="bg-white rounded-lg p-3 border border-amber-100">
                                <div className="flex items-center gap-2 text-sm">
                                    <span className="font-medium text-gray-700">Server:</span>
                                    <code className="bg-gray-100 px-1.5 py-0.5 rounded text-gray-800">{call.server}</code>
                                </div>
                                <div className="flex items-center gap-2 text-sm mt-1">
                                    <span className="font-medium text-gray-700">Tool:</span>
                                    <code className="bg-purple-100 px-1.5 py-0.5 rounded text-purple-800">{call.tool}</code>
                                </div>
                                {Object.keys(call.arguments).length > 0 && (
                                    <details className="mt-2">
                                        <summary className="text-xs text-gray-500 cursor-pointer hover:text-gray-700">
                                            View arguments
                                        </summary>
                                        <pre className="mt-1 text-xs bg-gray-50 p-2 rounded overflow-x-auto">
                                            {JSON.stringify(call.arguments, null, 2)}
                                        </pre>
                                    </details>
                                )}
                            </div>
                        ))}
                    </div>
                    <div className="flex gap-3">
                        <button
                            onClick={onApprove}
                            className="px-4 py-2 bg-green-600 text-white rounded-lg text-sm font-medium hover:bg-green-700 transition-colors"
                        >
                            ✓ Approve
                        </button>
                        <button
                            onClick={onReject}
                            className="px-4 py-2 bg-gray-200 text-gray-700 rounded-lg text-sm font-medium hover:bg-gray-300 transition-colors"
                        >
                            ✕ Reject
                        </button>
                    </div>
                </div>
            </div>
        </div>
    );
};
