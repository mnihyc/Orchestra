/** One captured presentation/account action, shared with the existing consolidated save dialog. */
import { create } from 'zustand';
import { client } from '@/api/client';

interface CapturedAction { ids: readonly string[]; run: () => void; current: () => boolean }
export const useCapturedAction = create<{ action: CapturedAction | null }>(() => ({ action: null }));

/** The caller supplies current loss state so layout code need not import its dialog or create a store cycle. */
export function captureViewAction(ids: readonly string[], dirty: boolean, run: () => void, current: () => boolean = () => true): void {
  if (useCapturedAction.getState().action !== null || !current()) return;
  const reader = client.session.getState();
  const stillCurrent = () => {
    const now = client.session.getState();
    return now.userId === reader.userId && now.readerVersion === reader.readerVersion && now.status === reader.status && current();
  };
  if (!dirty) { run(); return; }
  useCapturedAction.setState({ action: { ids: [...ids], run, current: stillCurrent } });
}
client.session.subscribe((next, before) => {
  if (next.userId !== before.userId || next.readerVersion !== before.readerVersion || next.status !== before.status) useCapturedAction.setState({ action: null });
});
