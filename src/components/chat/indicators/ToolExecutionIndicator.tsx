import { useEffect, useState, useRef } from 'react';
import { formatSecondsAsTime } from '../utils';
import { StatusRow } from '../StatusRow';

interface ToolExecutionIndicatorProps {
    server: string;
    tool: string;
}

/** Live status line shown in the fixed footer area while a tool is executing. */
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
        <StatusRow
            label={`Running ${tool}`}
            details={[server, elapsed >= 1 ? formatSecondsAsTime(elapsed) : null]}
            isLive
        />
    );
};
