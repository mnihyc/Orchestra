/** A place-chat alias resolves through the same authorized native place reader as its existing view. */
import { nodeQuery, orgBySlugQuery } from '@/api/nodes';
import { queryClient } from '@/api/query';

export const chatAddress = (id: string, message?: string): string => `/chat/${encodeURIComponent(id)}${message === undefined || message === '' ? '' : `?message=${encodeURIComponent(message)}`}`;
export async function placeChatAddress(id: string, message?: string): Promise<string> {
  const identifier = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(id);
  const node = identifier ? await queryClient.query(nodeQuery(id)) : await queryClient.query(orgBySlugQuery(id));
  return chatAddress(node.chat_id, message);
}
