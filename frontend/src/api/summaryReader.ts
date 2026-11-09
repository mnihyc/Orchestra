/** The same current reader identity fences both full and batched paragraph projections. */
import { client } from '@/api/client';
export function summaryReaderKey(): string {
  const state = client.session.getState();
  return JSON.stringify([state.userId, state.readerVersion ?? 0, state.status]);
}
