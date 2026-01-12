import type { Message } from '../../../store/chat-store';

interface UserMessageProps {
    message: Message;
}

/**
 * User message bubble component
 */
export const UserMessage = ({ message }: UserMessageProps) => {
    return (
        <div className="flex justify-end px-4 md:px-8">
            <div className="max-w-[85%] bg-gray-100 text-gray-900 rounded-2xl px-4 py-2.5 text-[15px]">
                {message.content}
            </div>
        </div>
    );
};
