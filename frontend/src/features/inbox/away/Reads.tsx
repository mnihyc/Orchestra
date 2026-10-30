/** One captured group read/key and its returned Undo ticket, retained through lost replies and list resync. */
import { useQueryClient } from '@tanstack/react-query';
import { RefreshCw, Undo2 } from 'lucide-react';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { groupedKey, readGroup } from '@/api/away';
import { ApiError } from '@/api/client';
import type { ReadResult } from '@/features/settings/personal/away/types';
import { Button } from '@/ui/Button';
import { useErrorText } from '@/ui/errorText';
import { StateLine } from '@/ui/StateLine';
import type { PlaceGroup } from './model';

export interface GroupAct { key: string; groupKey: string; through: number; name: string; phase: 'reading' | 'unconfirmed' | 'failed' | 'read' | 'undoing' | 'undone' | 'expired' | 'invalid'; result: ReadResult | null; undoKey: string | null; error: unknown }

export function useGroupReads() {
  const client = useQueryClient();
  const [acts, setActs] = useState<ReadonlyMap<string, GroupAct>>(new Map());
  const held = useRef(new Map<string, GroupAct>());
  const set = (act: GroupAct) => { held.current.set(act.key, act); setActs(new Map(held.current)); };
  const reconcile = () => client.resetQueries({ queryKey: groupedKey });
  const send = async (act: GroupAct, undo = false) => {
    set({ ...act, phase: undo ? 'undoing' : 'reading', error: null });
    try {
      if (undo && !act.result?.undo_token) return;
      const result = await readGroup(undo ? { undo_token: act.result?.undo_token ?? '' } : { group_key: act.groupKey, through_seq: act.through }, undo ? act.undoKey ?? act.key : act.key);
      set({ ...act, phase: undo ? 'undone' : 'read', result: undo ? act.result : result, error: null });
    } catch (error) {
      const phase = undo && error instanceof ApiError && error.status === 410 ? 'expired' : undo && error instanceof ApiError && error.status === 400 ? 'invalid' : error instanceof ApiError && error.status < 500 ? 'failed' : 'unconfirmed';
      set({ ...act, phase, error });
    } finally { await reconcile(); }
  };
  const read = (group: PlaceGroup, name: string) => {
    const existing = [...held.current.values()].find((act) => act.groupKey === group.key && (act.phase === 'reading' || act.phase === 'unconfirmed' || act.phase === 'failed'));
    if (existing !== undefined) { if (existing.phase === 'unconfirmed' || existing.phase === 'failed') void send(existing, existing.undoKey !== null); return; }
    const act: GroupAct = { key: crypto.randomUUID(), groupKey: group.key, through: group.through_seq, name, phase: 'reading', result: null, undoKey: null, error: null };
    void send(act);
  };
  const retry = (act: GroupAct) => { const phase = held.current.get(act.key)?.phase; if (phase === 'unconfirmed' || phase === 'failed') void send(act, act.undoKey !== null); };
  const undo = (act: GroupAct) => {
    if (held.current.get(act.key)?.phase !== 'read' || act.result?.undo_token === null) return;
    void send({ ...act, undoKey: act.undoKey ?? crypto.randomUUID() }, true);
  };
  return { acts: [...acts.values()], read, retry, undo };
}

export function ReadReceipt({ act, onRetry, onUndo }: { act: GroupAct; onRetry: () => void; onUndo: () => void }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const pending = act.phase === 'reading' || act.phase === 'undoing';
  const label = act.phase === 'undone' ? t('away.inbox.undone') : act.phase === 'expired' ? t('away.inbox.expired') : act.phase === 'invalid' ? t('away.inbox.invalid') : act.phase === 'failed' ? t('away.inbox.readFailed') : act.phase === 'read' || act.phase === 'undoing' ? t('away.inbox.marked') : t('away.inbox.unconfirmed');
  const action = (act.phase === 'read' || act.phase === 'undoing') && act.result !== null && act.result.undo_token !== null
    ? <Button size="sm" icon={<Undo2 aria-hidden />} pending={pending} onClick={onUndo}>{t('away.inbox.undo')}</Button>
    : act.phase === 'unconfirmed' || act.phase === 'failed' || act.phase === 'reading'
      ? <Button size="sm" icon={<RefreshCw aria-hidden />} pending={pending} onClick={onRetry}>{t('away.retry')}</Button> : undefined;
  return <StateLine role="status" tone={act.error === null ? 'neutral' : 'warning'} title={label}
    message={act.error === null || act.phase === 'expired' || act.phase === 'invalid' ? undefined : describe(act.error).message} action={action} compact />;
}
