/** A numbered message reference needs the server's shared-reader number, independent of chat events. */
import type { ChatMessage } from '@/generated/api-types';
import { referenceTo } from '@/lib/links';

export function messageReferenceNumber(message: { reference_number?: unknown }): number | null {
  if (!('reference_number' in message)) return null;
  const number = message.reference_number;
  return typeof number === 'number' && Number.isSafeInteger(number) && number > 0 ? number : null;
}

/** A menu keeps the stored message's own chat and shared-reader ordinal, even in another view. */
export function messageReferenceText(message: ChatMessage, chatName: string): string | null {
  const number = messageReferenceNumber(message);
  return number === null ? null : referenceTo({ kind: 'chat', id: message.chat_id, name: chatName }, { number });
}
