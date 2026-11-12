/** The first local setup of a thread whose imported record has no executable binding. */
import { api, ok } from '@/api/client';
import type { Thread, ThreadBindingSetBody, ThreadEnvironmentSetBody } from '@/generated/api-types';

export function placeUnboundThread(id: string, body: ThreadEnvironmentSetBody): Promise<Thread> {
  return ok(api.PUT('/threads/{id}/environment', { params: { path: { id } }, body }));
}

export async function bindUnboundThread(id: string, body: ThreadBindingSetBody): Promise<Thread> {
  const result = await ok(api.PUT('/threads/{id}/binding', { params: { path: { id } }, body }));
  if ('projection' in result) {
    throw new Error('The binding was saved, but its details are no longer available.');
  }
  return result;
}
