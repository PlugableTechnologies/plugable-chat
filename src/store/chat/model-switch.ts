// Decisions about switching models while a chat is open, kept free of Tauri and the store so
// they run under `npm run test:unit`.

/**
 * Opening a chat from the sidebar used to force its saved model back over the one the user had
 * picked, so a switch made in one chat was silently undone by reopening another. The model the
 * user selected stays; this returns the saved model only when it differs, so the chat can offer
 * a "Use X" action instead.
 */
export function chatModelNotice(chatModel: string | null | undefined, currentModel: string): string | null {
    if (!chatModel || chatModel === currentModel) return null;
    return chatModel;
}

/**
 * Whether choosing `next` while `current` is loaded should end the open chat. It never does: the
 * thread, its attachments and its RAG context are not tied to a model, and the next message is
 * saved on the chat with the new model. Kept as a function so the rule has one named place and a
 * test that fails if the clear comes back.
 */
export function shouldEndChatOnModelSwitch(_current: string, _next: string): boolean {
    return false;
}

/**
 * Only an explicit `false` from the backend is a failed switch. `SetModel` used to answer `true`
 * without loading anything; it now answers `false` when the load fails, and a missing answer
 * (older backend) is not treated as a failure.
 */
export function setModelSucceeded(answer: unknown): boolean {
    return answer !== false;
}
