/** Automatic writes for small settings forms; every write/undo retains the opening value and native revision. */
import { useMutation, type QueryKey } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { TYPING_PAUSE_MS } from '@/lib/debounce';
import { ApiError } from '@/api/client';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SaveStatus } from '@/ui/SaveStatus';

export interface SavedValue<Value> { value: Value; revision?: number | string | null }
interface Change<Value> { value: Value; before: SavedValue<Value>; undo: boolean }
interface Receipt<Value> { before: Value; after: SavedValue<Value> }
function same<Value>(left: Value, right: Value): boolean { return JSON.stringify(left) === JSON.stringify(right); }
function sameSnapshot<Value>(left: SavedValue<Value>, right: SavedValue<Value>): boolean { return left.revision === right.revision && same(left.value, right.value); }

export function useAutoSave<Value>({ current, write, valid, enabled = true, blocked = false, mutationKey, reload }: {
  current: SavedValue<Value>;
  /** Apply only named fields; return the accepted value and revision from this one answer, updating its native cache. */
  write: (value: Value, before: SavedValue<Value>) => Promise<SavedValue<Value>>;
  valid?: (value: Value) => boolean;
  enabled?: boolean;
  blocked?: boolean;
  mutationKey?: QueryKey;
  /** Only an explicit conflict recovery reads again; opening/editing/saving never does. */
  reload?: () => Promise<unknown>;
}) {
  const [draft, setDraft] = useState<{ value: Value; before: SavedValue<Value> } | null>(null);
  const [receipt, setReceipt] = useState<Receipt<Value> | null>(null);
  const [feedback, setFeedback] = useState<'saved' | 'undone' | null>(null);
  const [composing, setComposing] = useState(false);
  const flight = useRef(false);
  const value = draft === null ? current.value : draft.value;
  const dirty = draft !== null && !same(draft.value, draft.before.value);
  const stale = draft !== null && !sameSnapshot(draft.before, current);
  const acceptable = valid?.(value) ?? true;
  const { mutate, reset, isPending, error, variables } = useMutation({
    mutationKey,
    mutationFn: (change: Change<Value>) => write(change.value, change.before),
    onSuccess: (accepted, change) => {
      // Typing may continue during the request. Queue the newer draft against this accepted answer;
      // never replace it with the earlier text or issue two writes at the same time.
      setDraft((held) => held === null || same(held.value, change.value) ? null : { value: held.value, before: accepted });
      setReceipt(change.undo || sameSnapshot(change.before, accepted) ? null : { before: change.before.value, after: accepted });
      setFeedback(change.undo ? 'undone' : 'saved');
    },
    onSettled: () => { flight.current = false; },
  });
  const send = useCallback((retry = false) => {
    if (draft === null || !dirty || stale || !acceptable || !enabled || blocked || composing || flight.current || isPending || (!retry && error !== null)) return;
    flight.current = true;
    mutate({ value: draft.value, before: draft.before, undo: false });
  }, [draft, dirty, stale, acceptable, enabled, blocked, composing, isPending, error, mutate]);
  const ready = dirty && !stale && acceptable && enabled && !blocked && !composing && !isPending && error === null;
  useEffect(() => {
    if (!ready) return;
    // Use the same rest interval as other typed fields; no request is made merely by opening a form.
    const timer = setTimeout(() => send(), TYPING_PAUSE_MS);
    return () => clearTimeout(timer);
  }, [send, ready]);
  const clearDraft = () => { setDraft(null); setReceipt(null); setFeedback(null); reset(); };
  const serverStale = error instanceof ApiError && error.is('conflict.stale_revision');
  const reconcile = useMutation({ mutationFn: async () => { await reload?.(); }, onSuccess: clearDraft });
  const edit = (next: Value | ((before: Value) => Value), immediately = false) => {
    if (!enabled || reconcile.isPending) return;
    if (!isPending && !flight.current) reset();
    setFeedback(null); setReceipt(null);
    const chosen = typeof next === 'function' ? (next as (before: Value) => Value)(value) : next;
    const before = draft?.before ?? current;
    setDraft({ value: chosen, before });
    if (immediately && !same(chosen, before.value) && sameSnapshot(before, current) && (valid?.(chosen) ?? true) && !blocked && !composing && !isPending && !flight.current) {
      flight.current = true; mutate({ value: chosen, before, undo: false });
    }
  };
  const canUndo = receipt !== null && sameSnapshot(receipt.after, current) && !dirty && !isPending && !composing && enabled && !blocked && error === null;
  const undo = () => {
    if (!canUndo || flight.current) return;
    flight.current = true;
    mutate({ value: receipt.before, before: receipt.after, undo: true });
  };
  const discard = () => {
    if (isPending || reconcile.isPending) return;
    if (serverStale && reload !== undefined) reconcile.mutate();
    else clearDraft();
  };
  const retry = () => {
    if (serverStale) return;
    if (variables?.undo === true && receipt !== null && sameSnapshot(receipt.after, current) && !dirty && !isPending && enabled && !blocked && !flight.current) {
      flight.current = true; mutate({ value: receipt.before, before: receipt.after, undo: true });
    } else send(true);
  };
  const retryAvailable = !serverStale && enabled && !blocked && !isPending && !reconcile.isPending && !composing &&
    (variables?.undo === true ? receipt !== null && sameSnapshot(receipt.after, current) && !dirty : dirty && !stale && acceptable);
  return { value, edit, choose: (next: Value | ((before: Value) => Value)) => edit(next, true), commit: () => send(), retry, composing: setComposing, pending: isPending || reconcile.isPending, reloading: reconcile.isPending,
    dirty, stale: stale || serverStale, error: serverStale ? reconcile.error ?? error : error, recoveryFailed: serverStale && reconcile.isError,
    discard, undo, canUndo, hasUndo: receipt !== null, retryAvailable,
    state: isPending || reconcile.isPending ? 'saving' as const : error !== null ? null : !dirty ? feedback : null };
}

/** The same quiet feedback follows every automatic settings form; failures keep the draft. */
export function AutoSaveFeedback({ save, name }: { name?: string | undefined; save: Pick<ReturnType<typeof useAutoSave>, 'state' | 'hasUndo' | 'canUndo' | 'undo' | 'stale' | 'discard' | 'error' | 'retry' | 'pending' | 'recoveryFailed' | 'retryAvailable'> }) {
  const { t } = useTranslation();
  return <>
    <SaveStatus state={save.state} name={name} onUndo={save.hasUndo ? save.undo : undefined} undoDisabled={!save.canUndo} />
    {save.stale && (save.recoveryFailed
      ? <ErrorAlert error={save.error} action={<Button size="sm" pending={save.pending} onClick={save.discard}>{t('settings.auto.latest')}</Button>} />
      : <InlineAlert tone="warning" action={<Button size="sm" pending={save.pending} onClick={save.discard}>{t('settings.auto.latest')}</Button>}>{t('settings.auto.changed')}</InlineAlert>)}
    {save.error !== null && !save.stale && <ErrorAlert error={save.error}
      action={<Button size="sm" onClick={save.retryAvailable ? save.retry : save.discard} pending={save.pending}>{t(save.retryAvailable ? 'common.tryAgain' : 'common.dismiss')}</Button>} />}
  </>;
}
