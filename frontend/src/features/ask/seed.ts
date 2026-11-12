/** Plain user draft context, never a source permission or a model-send instruction. */
import { createStore } from 'zustand/vanilla';
import { client } from '@/api/client';
import { useComposerStore } from '@/features/composer/Draft';

interface Seed { readonly id: string; readonly reader: string; readonly text: string; readonly root: string | null }
const reader = () => {
  const current = client.session.getState();
  return JSON.stringify([current.userId, current.readerVersion ?? 0, current.status]);
};
export const askSeed = createStore<{ seed: Seed | null }>(() => ({ seed: null }));
export function clearAskDraftSeed(): void { askSeed.setState({ seed: null }); }

/** The latest explicit opening waits in memory for this reader's normal Ask draft. */
export function queueAskDraft(text: string, root: string | null = null): void {
  if (text.trim() === '') { clearAskDraftSeed(); return; }
  askSeed.setState({ seed: { id: crypto.randomUUID(), reader: reader(), text, root } });
}

/** The panel supplies its actual resolved draft key after its normal root/conversation read. */
export function applyAskDraftSeed(id: string, draftKey: string, root?: string): boolean {
  const pending = askSeed.getState().seed;
  if (pending?.id !== id) return false;
  if (pending.reader !== reader()) { clearAskDraftSeed(); return false; }
  if (pending.root !== null && pending.root !== root) return false;
  askSeed.setState({ seed: null });
  useComposerStore.setState((before) => {
    const text = before.text[draftKey] ?? '';
    return { text: { ...before.text, [draftKey]: text === '' ? pending.text : `${text}\n\n${pending.text}` } };
  });
  return true;
}

client.onSession((event) => {
  if (event.type === 'signed_in' || event.type === 'signed_out') clearAskDraftSeed();
});
