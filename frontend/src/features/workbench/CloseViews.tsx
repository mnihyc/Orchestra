/** One loss decision for a whole close plan, with all affected content retained after any failed save. */
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { allTabs } from './layout';
import { useWorkbench } from './store';
import { captureViewAction, useCapturedAction } from './lossActions';

export interface ViewSaveGuard { save: () => Promise<boolean>; discard: () => void }
const guards = new Map<string, ViewSaveGuard>();
/** Retargeting and explicit sign-out use the same loss decision, without closing any tab. */
export function requestViewAction(ids: readonly string[], run: () => void, current: () => boolean = () => true): void {
  const state = useWorkbench.getState();
  if (state.pendingClose !== null) return;
  captureViewAction(ids, ids.some((id) => state.unsaved.has(id)), run, current);
}

/** Activity may pause a dirty body. Its captured guard remains until the actual tab is removed. */
export function useViewSaveGuard(id: string | undefined, guard: ViewSaveGuard): void {
  useEffect(() => {
    if (id === undefined) return;
    guards.set(id, guard);
    return () => {
      if (!allTabs(useWorkbench.getState().layout).some((tab) => tab.id === id) && guards.get(id) === guard) guards.delete(id);
    };
  }, [id, guard]);
}
useWorkbench.subscribe((next, before) => {
  if (next.pendingClose !== null && next.pendingClose !== before.pendingClose) useCapturedAction.setState({ action: null });
  if (next.layout === before.layout) return;
  const live = new Set(allTabs(next.layout).map((tab) => tab.id));
  for (const id of guards.keys()) if (!live.has(id)) guards.delete(id);
});

export function CloseViews() {
  const { t } = useTranslation();
  const closeIds = useWorkbench((state) => state.pendingClose);
  const action = useCapturedAction((state) => state.action);
  const ids = action?.ids ?? closeIds;
  const dirty = useWorkbench((state) => state.unsaved);
  const named = useWorkbench((state) => state.named);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<{ ids: readonly string[]; error: unknown; saving: boolean } | null>(null);
  const error = failure?.ids === ids ? failure.error : null;
  const cancel = () => {
    if (pending) return;
    if (action !== null) useCapturedAction.setState({ action: null });
    else useWorkbench.getState().cancelClose();
  };
  const close = async (save: boolean) => {
    if (ids === null || pending) return;
    const reader = client.session.getState();
    const current = () => {
      const now = client.session.getState();
      return now.userId === reader.userId && now.readerVersion === reader.readerVersion && now.status === reader.status
        && (action === null ? useWorkbench.getState().pendingClose === ids : useCapturedAction.getState().action === action && action.current());
    };
    setPending(true); setFailure(null);
    try {
      for (const id of ids) {
        if (!current()) return;
        if (!useWorkbench.getState().unsaved.has(id)) continue;
        const guard = guards.get(id);
        if (guard === undefined) throw new Error(t('workbench.workspace.closeFailed'));
        if (save) {
          if (!await guard.save()) throw new Error(t('workbench.workspace.closeFailed'));
        } else guard.discard();
      }
      // A different plan cannot be confirmed by the outcome of this one.
      if (!current()) return;
      if (action === null) useWorkbench.getState().finishClose(ids);
      else { action.run(); useCapturedAction.setState({ action: null }); }
    } catch (error) { setFailure({ ids, error, saving: save }); }
    finally { setPending(false); }
  };
  return <Dialog open={ids !== null} onOpenChange={(open) => { if (!open) cancel(); }} dismissible={!pending}
    title={t(action === null ? 'workbench.workspace.saveClose' : 'workbench.workspace.saveContinue')} footer={<>
      <Button onClick={cancel} disabled={pending}>{t('common.cancel')}</Button>
      <Button onClick={() => void close(false)} disabled={pending}>{t('workbench.workspace.discard')}</Button>
      <Button onClick={() => void close(true)} pending={pending}>{t('workbench.workspace.saveAll')}</Button>
    </>}>
    {error !== null && <ErrorAlert error={error} message={t(failure?.saving === false ? 'workbench.workspace.discardFailed' : 'workbench.workspace.closeFailed')} />}
    <ul>{ids?.filter((id) => dirty.has(id)).map((id) => <li key={id}>{named.get(id)?.title ?? t('views.screen.title')}</li>)}</ul>
  </Dialog>;
}
