import { useEffect, useState, useRef } from 'react';
import { formatSecondsAsTime } from '../utils';

interface ToolExecutionIndicatorProps {
    server: string;
    tool: string;
}

/**
 * Tool execution indicator component (shown in the fixed footer area)
 * Shows animated dots while a tool is being executed
 */
export const ToolExecutionIndicator = ({ server, tool }: ToolExecutionIndicatorProps) => {
    const [elapsed, setElapsed] = useState(0);
    const startTime = useRef(Date.now());

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime.current) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, []);

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" />
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '300ms' }} />
                <div className="w-1.5 h-1.5 bg-purple-400 rounded-full animate-pulse" style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                Executing tool <code className="bg-purple-100 px-1 py-0.5 rounded text-purple-700">{tool}</code> on {server}
                {elapsed >= 1 ? ` · ${formatSecondsAsTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};
