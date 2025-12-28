import { useEffect, useState } from 'react';
import { X } from 'lucide-react';
import { useChatStore, OperationStatus } from '../store/chat-store';

// Format elapsed time helper
const formatElapsedTime = (seconds: number): string => {
    if (seconds < 60) return `${seconds}s`;
    const mins = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${mins}m ${secs}s`;
};

// Get background color based on operation type
const getStatusBarColors = (status: OperationStatus) => {
    if (status.completed) {
        return {
            bg: 'bg-green-50',
            border: 'border-green-200',
            text: 'text-green-800',
            icon: 'text-green-600',
            progress: 'bg-green-500',
        };
    }
    
    switch (status.type) {
        case 'downloading':
            return {
                bg: 'bg-blue-50',
                border: 'border-blue-200',
                text: 'text-blue-800',
                icon: 'text-blue-600',
                progress: 'bg-blue-500',
            };
        case 'loading':
            return {
                bg: 'bg-amber-50',
                border: 'border-amber-200',
                text: 'text-amber-800',
                icon: 'text-amber-600',
                progress: 'bg-amber-500',
            };
        case 'streaming':
            return {
                bg: 'bg-purple-50',
                border: 'border-purple-200',
                text: 'text-purple-800',
                icon: 'text-purple-600',
                progress: 'bg-purple-500',
            };
        case 'reloading':
            return {
                bg: 'bg-red-50',
                border: 'border-red-300',
                text: 'text-red-800',
                icon: 'text-red-600',
                progress: 'bg-red-500',
            };
        case 'indexing':
            return {
                bg: 'bg-indigo-50',
                border: 'border-indigo-200',
                text: 'text-indigo-800',
                icon: 'text-indigo-600',
                progress: 'bg-indigo-500',
            };
        default:
            return {
                bg: 'bg-gray-50',
                border: 'border-gray-200',
                text: 'text-gray-800',
                icon: 'text-gray-600',
                progress: 'bg-gray-500',
            };
    }
};

// Get operation icon
const getOperationIcon = (status: OperationStatus) => {
    if (status.completed) {
        return '✓';
    }
    
    switch (status.type) {
        case 'downloading':
            return '⬇️';
        case 'loading':
            return '⚡';
        case 'streaming':
            return '💬';
        case 'reloading':
            return '🔄';
        case 'indexing':
            return '📂';
        default:
            return '⏳';
    }
};

export function StatusBar() {
    const { 
        operationStatus, 
        statusBarDismissed, 
        dismissStatusBar, 
        heartbeatWarningStart, 
        heartbeatWarningMessage,
        modelStuckWarning,
        setModelStuck
    } = useChatStore();
    const [elapsed, setElapsed] = useState(0);
    const [heartbeatElapsed, setHeartbeatElapsed] = useState(0);
    
    // Update elapsed time every second
    useEffect(() => {
        if (!operationStatus || operationStatus.completed) {
            setElapsed(0);
            return;
        }
        
        const updateElapsed = () => {
            setElapsed(Math.floor((Date.now() - operationStatus.startTime) / 1000));
        };
        
        updateElapsed();
        const interval = setInterval(updateElapsed, 1000);
        return () => clearInterval(interval);
    }, [operationStatus]);
    
    // Don't render if no operation or dismissed
    useEffect(() => {
        if (!heartbeatWarningStart) {
            setHeartbeatElapsed(0);
            return;
        }
        const updateElapsed = () => {
            setHeartbeatElapsed(Math.floor((Date.now() - heartbeatWarningStart) / 1000));
        };
        updateElapsed();
        const interval = setInterval(updateElapsed, 1000);
        return () => clearInterval(interval);
    }, [heartbeatWarningStart]);

    if (statusBarDismissed) {
        return null;
    }

    const heartbeatActive = !!heartbeatWarningStart;
    const modelStuckActive = !!modelStuckWarning;
    const colors = operationStatus ? getStatusBarColors(operationStatus) : null;
    const icon = operationStatus ? getOperationIcon(operationStatus) : null;

    return (
        <>
            {heartbeatActive && (
                <div className="heartbeat-warning-bar flex items-center justify-between px-4 py-2 bg-red-50 border-b border-red-300 text-red-800">
                    <div className="flex items-center gap-3 flex-1 min-w-0">
                        <span className="text-lg">🚨</span>
                        <span className="text-sm font-medium truncate">
                            {heartbeatWarningMessage || 'Backend unresponsive'}
                        </span>
                        <div className="flex-shrink-0 text-xs text-red-700 opacity-80 font-mono">
                            {formatElapsedTime(heartbeatElapsed)}
                        </div>
                    </div>
                    <button
                        onClick={dismissStatusBar}
                        className="flex-shrink-0 ml-3 p-1 rounded-full hover:bg-black/5 transition-colors text-red-600"
                        aria-label="Dismiss heartbeat warning"
                    >
                        <X size={16} />
                    </button>
                </div>
            )}

            {modelStuckActive && (
                <div className="model-stuck-bar flex items-center justify-between px-4 py-2 bg-amber-50 border-b border-amber-200 text-amber-800">
                    <div className="flex items-center gap-3 flex-1 min-w-0">
                        <span className="text-lg">🛑</span>
                        <span className="text-sm font-medium">
                            {modelStuckWarning}
                        </span>
                    </div>
                    <button
                        onClick={() => setModelStuck(null)}
                        className="flex-shrink-0 ml-3 p-1 rounded-full hover:bg-black/5 transition-colors text-amber-600"
                        aria-label="Dismiss stuck warning"
                    >
                        <X size={16} />
                    </button>
                </div>
            )}

            {operationStatus && (
                <div className={`status-bar flex items-center justify-between px-4 py-2 ${colors!.bg} border-b ${colors!.border} transition-all`}>
                    <div className="flex items-center gap-3 flex-1 min-w-0">
                        {/* Icon/spinner */}
                        <div className={`flex-shrink-0 ${colors!.icon}`}>
                            {operationStatus.completed ? (
                                <span className="text-lg">{icon}</span>
                            ) : (
                                <div className="flex items-center">
                                    <span className="text-lg mr-1">{icon}</span>
                                    <div className="flex gap-0.5">
                                        <div className={`w-1 h-1 ${colors!.progress} rounded-full animate-pulse`} />
                                        <div className={`w-1 h-1 ${colors!.progress} rounded-full animate-pulse`} style={{ animationDelay: '200ms' }} />
                                        <div className={`w-1 h-1 ${colors!.progress} rounded-full animate-pulse`} style={{ animationDelay: '400ms' }} />
                                    </div>
                                </div>
                            )}
                        </div>
                        
                        {/* Message */}
                        <div className={`flex-1 min-w-0 ${colors!.text}`}>
                            <span className="text-sm font-medium truncate">
                                {operationStatus.message}
                            </span>
                            
                        {/* Progress info for downloads and indexing */}
                        {(operationStatus.type === 'downloading' || operationStatus.type === 'indexing') && operationStatus.currentFile && !operationStatus.completed && (
                            <span className="ml-2 text-xs opacity-75">
                                ({operationStatus.currentFile})
                            </span>
                        )}
                    </div>
                    
                    {/* Progress bar for downloads and indexing */}
                    {(operationStatus.type === 'downloading' || operationStatus.type === 'indexing') && operationStatus.progress !== undefined && !operationStatus.completed && (
                        <div className="w-32 h-2 bg-gray-200 rounded-full overflow-hidden flex-shrink-0">
                                <div 
                                    className={`h-full ${colors!.progress} transition-all duration-300`}
                                    style={{ width: `${operationStatus.progress}%` }}
                                />
                            </div>
                        )}
                        
                        {/* Elapsed time */}
                        {!operationStatus.completed && (
                            <div className={`flex-shrink-0 text-xs ${colors!.text} opacity-75 font-mono`}>
                                {formatElapsedTime(elapsed)}
                            </div>
                        )}
                        
                        {/* Completed indicator */}
                        {operationStatus.completed && (
                            <span className={`flex-shrink-0 text-xs font-medium ${colors!.text} px-2 py-0.5 rounded-full bg-green-100`}>
                                Complete
                            </span>
                        )}
                    </div>
                    
                    {/* Dismiss button */}
                    <button
                        onClick={dismissStatusBar}
                        className={`status-dismiss-button flex-shrink-0 ml-3 p-1 rounded-full hover:bg-black/5 transition-colors ${colors!.icon}`}
                        aria-label="Dismiss status"
                    >
                        <X size={16} />
                    </button>
                </div>
            )}
        </>
    );
}

// Warning status bar for when streaming is active in another chat
export function StreamingWarningBar() {
    const { streamingChatId, currentChatId, dismissStatusBar, history } = useChatStore();
    
    // Only show if streaming in a different chat
    if (!streamingChatId || streamingChatId === currentChatId) {
        return null;
    }
    
    // Find the name of the streaming chat
    const streamingChat = history.find(chat => chat.id === streamingChatId);
    const chatName = streamingChat?.title || 'another chat';
    
    return (
        <div className="streaming-warning-bar flex items-center justify-between px-4 py-2 bg-amber-50 border-b border-amber-200">
            <div className="flex items-center gap-3 flex-1 min-w-0">
                <span className="text-lg">⚠️</span>
                <span className="text-sm font-medium text-amber-800 truncate">
                    Response still streaming in "{chatName}". New messages are blocked until streaming completes.
                </span>
            </div>
            <button
                onClick={dismissStatusBar}
                className="flex-shrink-0 ml-3 p-1 rounded-full hover:bg-black/5 transition-colors text-amber-600"
                aria-label="Dismiss warning"
            >
                <X size={16} />
            </button>
        </div>
    );
}

