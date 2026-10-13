import type { ReactNode } from 'react';

interface StatusRowProps {
    label: string;
    /** Secondary facts shown after the label, joined with a middle dot. */
    details?: Array<string | null | undefined | false>;
    tone?: 'neutral' | 'error';
    /** Pulses the text while work is in flight. */
    isLive?: boolean;
    defaultOpen?: boolean;
    onToggle?: (isOpen: boolean) => void;
    /** Omit for a non-expandable status line. */
    children?: ReactNode;
}

/**
 * One quiet line of status (system prompt, reasoning, tool calls) that expands to details.
 * Deliberately has no box, icon or color so it reads as metadata next to the answer.
 */
export const StatusRow = ({ label, details = [], tone = 'neutral', isLive = false, defaultOpen, onToggle, children }: StatusRowProps) => {
    const toneClass = tone === 'error' ? 'text-red-500' : 'text-gray-400 hover:text-gray-600';
    const text = [label, ...details.filter(Boolean)].join(' · ');
    const textClass = `${isLive ? 'animate-pulse' : ''}`;

    if (!children) {
        return <div className={`my-0.5 text-xs leading-4 ${toneClass} ${textClass}`}>{text}</div>;
    }

    return (
        <details
            className="group/status my-0.5"
            open={defaultOpen}
            onToggle={onToggle ? (e) => onToggle((e.currentTarget as HTMLDetailsElement).open) : undefined}
        >
            <summary
                className={`flex cursor-pointer select-none list-none items-center gap-1 text-xs leading-4 transition-colors [&::-webkit-details-marker]:hidden ${toneClass}`}
            >
                <span className={textClass}>{text}</span>
                <span className="inline-block text-[11px] opacity-0 transition-all group-hover/status:opacity-100 group-open/status:rotate-90 group-open/status:opacity-100">
                    ›
                </span>
            </summary>
            <div className="my-1 ml-1 border-l border-gray-200 pl-3 text-xs text-gray-500">{children}</div>
        </details>
    );
};
