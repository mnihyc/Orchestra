/** What the end-to-end tests of the Inbox, the approvals and the ways a person is told share: the controls of the fake API that put notifications and approvals into it and steer the ways out. */
import type { Approval, Notification, NotificationKind } from '../../src/generated/api-types.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function control<Reply = unknown>(path: string, body: unknown = {}): Promise<Reply> {
  const response = await fetch(`${DOUBLE}/__double/notifications${path}`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
  return (response.status === 204 ? null : await response.json()) as Reply;
}

export interface Told {
  kind: NotificationKind;
  title: string;
  body?: string;
  /** The person; Ada unless said. */
  user?: string;
  thread_id?: string;
  node?: string;
  approval_id?: string;
  chat_id?: string;
  url?: string;
  /** When it was written. */
  at?: string;
  read?: boolean;
  /** Keeps the number and sends no frame, as a lost frame does. */
  lose?: boolean;
}

/** The system writes a row into somebody's Inbox, and their open pages are told. */
export function notify(row: Told): Promise<Notification> {
  return control<Notification>('/row', row);
}

/** Many rows a minute apart, the last of them at `at` or now. */
export function notifyMany(count: number, more: { user?: string; kind?: NotificationKind; read?: boolean; at?: string } = {}): Promise<{ unread: number }> {
  return control('/many', { count, ...more });
}

/** The Inbox the fake API holds for somebody, and its counters. */
export function inboxHeld(user = 'ada'): Promise<{ summary: { unread: number; approvals_pending: number; seq: number }; rows: Notification[]; followers: number }> {
  return control('/state', { user });
}

export interface Asked {
  id?: string;
  requester?: string;
  title?: string;
  /** Who may decide it; Ada unless said. */
  deciders?: string[];
  /** Who would see it only under "everything I could decide". */
  also?: string[];
  [more: string]: unknown;
}

export function askCapability(more: Asked & { risk?: 'low' | 'medium' | 'high' | 'critical'; reason?: string; consequences?: string[] } = {}): Promise<Approval> {
  return control<Approval>('/approval/capability', more);
}

export function askAccess(more: Asked & { batch_id?: string; permissions?: string[]; reason?: string; object?: unknown; for_thread?: unknown; options?: unknown; expires_at?: string } = {}): Promise<Approval> {
  return control<Approval>('/approval/access', more);
}

/** Somebody else decides an approval behind the page's back; `silent` tells nobody, so that a page finds out only by its own try. */
export function settleAs(id: string, by: string, state: 'approved' | 'denied' = 'approved', silent = false): Promise<Approval> {
  return control<Approval>('/approval/settle', { id, by, state, silent });
}

/** What was made by `askCapability` and `askAccess` and what became of it. */
export function approvalsMade(): Promise<Approval[]> {
  return control<Approval[]>('/approval/state');
}

/** The person presses Start in the bot: their route is linked, and their feed says so. */
export function botStarted(label = '@alice_c', user = 'ada'): Promise<null> {
  return control<null>('/route/linked', { user, label });
}

/** Which ways out this system has, by the kinds of its notifiers; webhooks of a person's own stay. */
export function onlyChannels(kinds: string[]): Promise<null> {
  return control<null>('/channels', { only: kinds });
}

/** Whether somebody's address is confirmed. */
export function emailConfirmed(confirmed: boolean, user = 'ada'): Promise<null> {
  return control<null>('/mail/verified', { user, verified: confirmed });
}
