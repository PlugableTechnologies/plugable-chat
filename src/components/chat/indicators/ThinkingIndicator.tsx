import { useEffect, useState } from 'react';
import { formatSecondsAsTime } from '../utils';

interface ThinkingIndicatorProps {
    startTime: number;
}

/**
 * Thinking indicator component with elapsed time
 * Shows animated dots and elapsed time while the model is reasoning
 */
export const ThinkingIndicator = ({ startTime }: ThinkingIndicatorProps) => {
    const [elapsed, setElapsed] = useState(0);

    useEffect(() => {
        const interval = setInterval(() => {
            setElapsed(Math.floor((Date.now() - startTime) / 1000));
        }, 1000);
        return () => clearInterval(interval);
    }, [startTime]);

    return (
        <div className="flex items-center gap-2 text-xs text-gray-500 mt-2 mb-1">
            <div className="flex gap-1">
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" />
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" style={{ animationDelay: '300ms' }} />
                <div className="w-1.5 h-1.5 bg-amber-400 rounded-full animate-pulse" style={{ animationDelay: '600ms' }} />
            </div>
            <span className="font-medium text-gray-500">
                Reasoning{elapsed >= 1 ? ` · ${formatSecondsAsTime(elapsed)}` : '...'}
            </span>
        </div>
    );
};
