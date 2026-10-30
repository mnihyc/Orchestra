/** Explicit person notices and the actions available beside one readable thing. */
import { queryOptions } from '@tanstack/react-query';
import { api, done, ok, repeatable } from '@/api/client';
import { changePage, pathOf } from '@/api/access';
import type { AccessChoice } from '@/generated/api-types';
import type { PersonAnchor, PersonThing } from '@/features/people/personContext';

export interface PersonActions {
  can_view: boolean | null;
  can_notify: boolean;
  gives: AccessChoice[];
  note_chars: number;
}

interface Answer<T> { data?: T; error?: unknown; response: Response }
type ContextRead = (path: '/{object}/{id}/notify/people/{user_id}', options: {
  params: { path: { object: string; id: string; user_id: string } }; signal?: AbortSignal;
}) => Promise<Answer<PersonActions>>;
type NoticeWrite = (path: '/{object}/{id}/notify', options: {
  params: { path: { object: string; id: string } };
  body: { user_ids: string[]; anchor?: PersonAnchor; note?: string };
  headers: Record<string, string>;
}) => Promise<Answer<never>>;
type ChatGrantWrite = (path: '/chats/{id}/permissions/people/{user_id}', options: {
  params: { path: { id: string; user_id: string } };
  body: { choice: AccessChoice; notify: boolean; anchor?: PersonAnchor };
  headers: Record<string, string>;
}) => Promise<Answer<never>>;

export const personActionsQuery = (thing: PersonThing, userId: string) => queryOptions({
  queryKey: ['person-actions', pathOf(thing.kind), thing.id, userId],
  // One bounded context read belongs to the query cache, including a StrictMode remount.
  // The session's cache fence and every mutation's current authority checks still apply.
  queryFn: (): Promise<PersonActions> => ok((api.GET as unknown as ContextRead)('/{object}/{id}/notify/people/{user_id}', {
    params: { path: { object: pathOf(thing.kind), id: thing.id, user_id: userId } },
  })),
  staleTime: 0,
});

export function notifyPerson(thing: PersonThing, userId: string, note: string, key: string): Promise<void> {
  return done((api.POST as unknown as NoticeWrite)('/{object}/{id}/notify', {
    params: { path: { object: pathOf(thing.kind), id: thing.id } },
    body: { user_ids: [userId], ...(thing.anchor === undefined ? {} : { anchor: thing.anchor }), ...(note === '' ? {} : { note }) },
    ...repeatable(key),
  }));
}

export async function grantPerson(thing: PersonThing, userId: string, choice: AccessChoice, notify: boolean, key: string): Promise<void> {
  if (thing.kind === 'chat') {
    await done((api.POST as unknown as ChatGrantWrite)('/chats/{id}/permissions/people/{user_id}', {
      params: { path: { id: thing.id, user_id: userId } },
      body: { choice, notify, ...(thing.anchor === undefined ? {} : { anchor: thing.anchor }) },
      ...repeatable(key),
    }));
    return;
  }
  await changePage(thing, {
    name: [{ holders: [{ kind: 'user', id: userId }], choice, export: false }],
    ...(notify ? { notify: true, ...(thing.anchor === undefined ? {} : { notify_anchor: thing.anchor }) } : {}),
  }, key);
}
