/** Marking the rows of the Inbox read: one, those of a place or a chat, or all.
 *
 * A read mark is the person's own and easily undone, so it shows at once and is put back if the server
 * refuses. What the server says is unread afterwards (the number it answers with) is the count; the rows
 * it changed come back on the person's feed with new Inbox numbers, so every device learns.
 */
import { markRead } from '@/api/notifications';
import type { Notification, NotificationReadBody } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { placeOf } from '@/features/inbox/model';
import { userWriter, useUserStore } from '@/realtime/stores/user';
import { errorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

export type ReadSelection =
  /** These rows. */
  | { ids: readonly string[] }
  /** Whatever the rows say they are about, in this place (a thread, a topic, a project). */
  | { place: string }
  /** The rows of this chat. */
  | { chat: string }
  /** Every row. */
  | { all: true };

export interface ReadOptions {
  /** Asks the server even where no row held is unread, since older rows the pages have not reached may be. */
  ask?: boolean;
}

/** The unread rows held that a selection names. */
export function unreadIn(rows: Iterable<Notification>, selection: ReadSelection): Notification[] {
  const wanted = 'ids' in selection ? new Set(selection.ids) : null;
  const unread: Notification[] = [];
  for (const row of rows) {
    if (row.read_at !== null) continue;
    if (wanted !== null) {
      if (!wanted.has(row.id)) continue;
    } else if ('place' in selection) {
      if (placeOf(row) !== selection.place) continue;
    } else if ('chat' in selection) {
      if (row.chat_id !== selection.chat) continue;
    }
    unread.push(row);
  }
  return unread;
}

/** What to say to the server for a selection. `now` is the moment "all" counts up to, or later when a row was written later. */
export function bodyOf(selection: ReadSelection, newest: string | null, now: number): NotificationReadBody {
  if ('ids' in selection) return { ids: [...selection.ids] };
  if ('place' in selection) return { node_id: selection.place };
  if ('chat' in selection) return { chat_id: selection.chat };
  // Every row written until now, and until the newest row held where this machine's clock is behind the server's.
  const until = Math.max(now, newest === null ? 0 : Date.parse(newest));
  return { before: new Date(until).toISOString() };
}

/**
 * Marks read what the selection names. The rows show as read at once; the count is the server's answer.
 * A refusal puts the rows back and says so.
 */
export async function markNotificationsRead(selection: ReadSelection, options: ReadOptions = {}): Promise<void> {
  const state = useUserStore.getState();
  const held = unreadIn(state.rows.values(), selection);
  if (held.length === 0) {
    // Nothing held is unread. "All" still counts what the pages have not reached; a place or a chat may be asked
    // once, when the person goes to it, for the same reason; nothing else has anything to ask.
    // An individual read after a group read must reach the server, so an older Undo leaves it read.
    const ask = 'ids' in selection ? selection.ids.length > 0 : 'all' in selection ? state.counters.unread > 0 : options.ask === true && !state.reachedEnd;
    if (!ask) return;
  }
  const at = new Date().toISOString();
  const before = userWriter.markRead(
    held.map((row) => row.id),
    at,
  );
  let newest: string | null = null;
  for (const row of state.rows.values()) if (newest === null || row.created_at > newest) newest = row.created_at;
  try {
    const answer = await markRead(bodyOf(selection, newest, Date.now()));
    userWriter.counters({ unread: answer.unread });
  } catch (error) {
    userWriter.unmarkRead(before);
    const text = errorText(error, i18n.t, {
      locale: useAppearance.getState().locale,
      timeZone: useAppearance.getState().timeZone,
    });
    toast({ tone: 'danger', title: i18n.t('inbox.notMarked'), description: text.message });
  }
}
