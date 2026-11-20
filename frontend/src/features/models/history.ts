/** Current selectable identities are the only rows a private history can promote. */
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { modelHistoryKey, modelHistoryQuery, recordModelChoice } from '@/api/modelHistory';
import type { ModelRecent, ModelRecents } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { newActKey } from '@/lib/requests';
import { toast } from '@/ui/Toast';

const keyOf = (model: ModelRecent) => JSON.stringify([model.provider_id, model.model_name]);

/** One promotion per row; entries not promoted retain their exact source order. */
export function splitRecent<T>(rows: readonly T[], history: ModelRecents | undefined, identity: (row: T) => ModelRecent | null): { recent: T[]; rest: T[] } {
  const used = new Set<number>(), recent: T[] = [];
  const count = history?.recent_count ?? 0;
  if (count <= 0 || history === undefined || history.recent.length === 0) return { recent, rest: [...rows] };
  const candidates = new Map<string, { rows: number[]; next: number }>();
  rows.forEach((row, at) => {
    const model = identity(row);
    if (model === null) return;
    const key = keyOf(model), held = candidates.get(key) ?? { rows: [], next: 0 };
    held.rows.push(at); candidates.set(key, held);
  });
  for (const model of history.recent) {
    if (recent.length >= count) break;
    const held = candidates.get(keyOf(model)), at = held?.rows[held.next];
    if (held === undefined || at === undefined) continue;
    const row = rows[at]; held.next += 1;
    if (row !== undefined) { recent.push(row); used.add(at); }
  }
  return { recent, rest: rows.filter((_, at) => !used.has(at)) };
}

/** Failures retain an explicit Retry without making a successful work change fail. */
export function rememberModel(cache: QueryClient, person: string | null, model: ModelRecent): void {
  if (person === null || client.session.getState().userId !== person) return;
  const generation = client.session.getState().readerVersion;
  const key = newActKey();
  const current = () => client.session.getState().userId === person && client.session.getState().readerVersion === generation;
  let dismiss: (() => void) | undefined;
  const save = () => {
    if (!current()) return;
    void recordModelChoice(model, key).then(() => {
      if (!current()) return;
      dismiss?.();
      // Responses can arrive out of order. Re-read the server's accepted order.
      void cache.invalidateQueries({ queryKey: modelHistoryKey(person) });
    }).catch(() => {
      if (!current()) return;
      dismiss?.();
      dismiss = toast({ tone: 'danger', title: i18n.t('modelPicker.historySaveFailed'),
        action: { label: i18n.t('common.tryAgain'), run: save } });
    });
  };
  save();
}

export function useModelHistory(open: boolean) {
  const person = useStore(client.session, (state) => state.status === 'signed_in' ? state.userId : null);
  const cache = useQueryClient();
  const read = useQuery({ ...modelHistoryQuery(person), enabled: open && person !== null });
  return { history: person === null ? undefined : read.data, read, remember: (model: ModelRecent) => rememberModel(cache, person, model) };
}
