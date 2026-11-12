/** Local Ask drafts keep one identity while the server allocates their private conversation. */
import { client } from '@/api/client';
import { useComposerStore } from './Draft';
import type { AskCreateBody } from '@/generated/api-types';

const newDrafts = new Map<string, string>();
const aliases = new Map<string, string>();
const conversations = new Map<string, string>();
const creations = new Map<string, { key: string; body: AskCreateBody }>();
interface AskSendIntent { content: string; id: string; send: () => Promise<void> }
const askAttempts = new Map<string, AskSendIntent>();
const ownerRoot = (root: string) => { const reader = client.session.getState(); return `ask:${reader.userId ?? ''}:${reader.readerVersion}:${root}`; };

export function askDraftKey(root: string, conversation: string | null): string {
  const owner = ownerRoot(root);
  if (conversation !== null) return aliases.get(`${owner}:${conversation}`) ?? `${owner}:${conversation}`;
  let key = newDrafts.get(owner);
  if (key === undefined) { key = `${owner}:new:${crypto.randomUUID()}`; newDrafts.set(owner, key); }
  return key;
}
export function newAskDraft(root: string): string {
  const key = `${ownerRoot(root)}:new:${crypto.randomUUID()}`; newDrafts.set(ownerRoot(root), key); return key;
}
export function allocatedAskDraft(key: string): string | undefined { return conversations.get(key); }
export function askCreationIntent(key: string, body: AskCreateBody): { key: string; body: AskCreateBody } {
  let intent = creations.get(key); if (intent === undefined) { intent = { key: crypto.randomUUID(), body }; creations.set(key, intent); } return intent;
}
export function registerAskDraft(root: string, id: string, key: string): void {
  aliases.set(`${ownerRoot(root)}:${id}`, key); conversations.set(key, id); creations.delete(key);
}
/** An uncertain send retains both identity and its original callback/body, including its captured destination. */
export function askSendIntent(key: string, fingerprint: string, send: (id: string) => Promise<void>): AskSendIntent {
  let attempt = askAttempts.get(key);
  if (attempt?.content !== fingerprint) {
    const id = crypto.randomUUID(); attempt = { content: fingerprint, id, send: () => send(id) }; askAttempts.set(key, attempt);
  }
  return attempt;
}
/** A confirmed send ends that intent. A late response cannot clear a newer edit at the same place. */
export function completeAskSendIntent(key: string, intent: AskSendIntent): void {
  if (askAttempts.get(key) === intent) askAttempts.delete(key);
}
client.onSession((event) => {
  if (event.type !== 'signed_in' && event.type !== 'signed_out') return;
  newDrafts.clear(); aliases.clear(); conversations.clear(); creations.clear(); askAttempts.clear();
  useComposerStore.setState((before) => ({
    text: Object.fromEntries(Object.entries(before.text).filter(([key]) => !key.startsWith('ask:'))),
    parts: Object.fromEntries(Object.entries(before.parts).filter(([key]) => !key.startsWith('ask:'))),
  }));
});
