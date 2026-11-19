/** Rows of a chat as the real API answers them: a chat, a message, a person's state in a chat; made with the defaults of an ordinary one, for the fake API and for the unit tests. */
import type { Chat, ChatMessage, ChatState, UserRef } from '../../src/generated/api-types.ts';

/** The moment the rows are "written" at, plus a number of seconds: fixed, so that nothing depends on the day a test runs. */
export function at(seconds: number): string {
  return new Date(Date.UTC(2026, 9, 1, 9, 0, 0) + seconds * 1_000).toISOString();
}

export function person(n: number, username: string, displayName: string): UserRef {
  return {
    id: `00000000-0000-4000-8001-${n.toString(16).padStart(12, '0')}`,
    kind: 'human',
    username,
    display_name: displayName,
    avatar: { text: Array.from(displayName)[0]?.toUpperCase() ?? '?', color: n % 12 },
  };
}

export const ADA = person(1, 'ada', 'Ada Lovelace');
export const GRACE = person(2, 'grace', 'Grace Hopper');
export const DORA = person(9, 'dora', 'Dora Developer');

/** A message with the stamps a chat would give it at `seq`; the members given replace the defaults. */
export function message(seq: number, more: Partial<ChatMessage> = {}): ChatMessage {
  return {
    mention_names: [],
    id: `00000000-0000-4000-8c00-${seq.toString(16).padStart(12, '0')}`,
    chat_id: '00000000-0000-4000-8009-000000000001',
    kind: 'message',
    author: ADA,
    parts: [{ type: 'text', text: `message ${seq}` }],
    reply_to: null,
    mentions: [],
    mentions_all: false,
    mention_targets: [],
    asked_agents: [],
    reactions: [],
    pinned: null,
    deleted: false,
    system: null,
    edited_at: null,
    created_at: at(seq),
    seq_created: seq,
    seq_updated: seq,
    ...more,
  };
}

export function chat(more: Partial<Chat> = {}): Chat {
  return {
    id: '00000000-0000-4000-8009-000000000001',
    kind: 'node',
    org_id: '00000000-0000-4000-8002-000000000064',
    node: { id: '00000000-0000-4000-8003-000000000003', kind: 'topic', name: 'Fix login', redacted: false },
    name: null,
    members: null,
    guests: [],
    created_by: ADA,
    created_at: at(0),
    seq: 0,
    last_message_seq: 0,
    last_message_at: null,
    preview: null,
    can: { admin_only: [], participating: true, send: true, moderate: false, export: true, reason: null },
    my: null,
    ...more,
  };
}

export function state(chatId: string, more: Partial<ChatState> = {}): ChatState {
  return {
    chat_id: chatId,
    effective_level: 'all',
    engaged: true,
    hidden: false,
    last_read_seq: 0,
    read_revision: 0,
    level: null,
    mentions_unread: 0,
    ...more,
  };
}
