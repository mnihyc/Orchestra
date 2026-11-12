/** The composing context of an explicit gift; the server supplies its actor. */
export type LinkOrigin = { kind: 'chat'; chat_id: string } | { kind: 'thread_input'; thread_id: string };
