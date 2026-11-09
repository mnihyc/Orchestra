/** The native home writer changes placement and inheritance, without making another identity. */
import { duplicateObjects } from '@/api/duplicates';
import { accepted, api, repeatable } from '@/api/client';
import { traceJson } from '@/api/trace';
import type { ChatChangeResponse } from '@/generated/api-types';
import type { HomeMoveKind } from '@/features/duplicate/move';
export interface HomeMoveBody { home_id: string; expected_revision?: number }
export interface HomeMoveReceipt { projection: 'receipt'; object_id: string; kind: string; home_id: string; changed: boolean }
const objects: Record<HomeMoveKind, string> = { ...duplicateObjects, host: 'hosts', machine: 'machines' };
export async function moveResourceHome(kind: HomeMoveKind, id: string, body: HomeMoveBody, key: string): Promise<HomeMoveReceipt | { chat_id: string }> {
  if (kind === 'chat') {
    const reply = await groupHome(id, body.home_id, key);
    const chatId = 'projection' in reply ? reply.chat_id : reply.id;
    if (chatId !== id) throw new Error('The home acknowledgement did not identify the accepted group.');
    return { chat_id: chatId };
  }
  const receipt = await traceJson<unknown>(`/${objects[kind]}/${encodeURIComponent(id)}/home`, { method: 'PUT', body, key });
  if (typeof receipt !== 'object' || receipt === null || !('projection' in receipt) || receipt.projection !== 'receipt' ||
      !('object_id' in receipt) || receipt.object_id !== id || !('kind' in receipt) || receipt.kind !== kind ||
      !('home_id' in receipt) || receipt.home_id !== body.home_id || !('changed' in receipt) || typeof receipt.changed !== 'boolean') throw new Error('The home acknowledgement did not identify the accepted move.');
  return { projection: receipt.projection, object_id: receipt.object_id, kind, home_id: receipt.home_id, changed: receipt.changed };
}

function groupHome(id: string, home: string, key: string): Promise<ChatChangeResponse> {
  return accepted(api.PATCH('/chats/{id}', { params: { path: { id } }, body: { home_id: home }, ...repeatable(key) }));
}
