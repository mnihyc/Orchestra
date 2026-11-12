/** A single reader-bound entrance, with operation dialogs loaded only when asked for. */
import { Suspense, useEffect } from 'react';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { client } from '@/api/client';
import { sameLibraryReader } from '@/api/resourceLibrary';
import { lazyPart } from '@/features/files/lazy';
import type { HomeMoveTarget } from './move';
import { useDuplicateHost, type DuplicateTarget } from './target';

const DuplicateDialog = lazyPart<{ target: DuplicateTarget; quick: boolean; onClose: () => void }>(() => import('./DuplicateDialog').then((module) => ({ default: module.DuplicateDialog })));
const ThreadDuplicate = lazyPart<{ target: DuplicateTarget; quick: boolean; onClose: () => void }>(() => import('./ThreadDuplicate').then((module) => ({ default: module.ThreadDuplicate })));
const MoveHomeDialog = lazyPart<{ target: HomeMoveTarget; onClose: () => void }>(() => import('./MoveHomeDialog').then((module) => ({ default: module.MoveHomeDialog })));
export function DuplicateHost() {
  const { t } = useTranslation();
  const current = useDuplicateHost((state) => state.current);
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  useEffect(() => {
    if (current !== null && !sameLibraryReader(current.reader)) useDuplicateHost.getState().close();
  }, [current, reader]);
  if (current === null || !sameLibraryReader(current.reader)) return null;
  if (current.mode === 'move') return <Suspense fallback={<Dialog open title={t('duplicate.moveTitle', { name: current.target.name })} onOpenChange={(open) => { if (!open) useDuplicateHost.getState().close(); }}><SkeletonRows rows={1} /></Dialog>}><MoveHomeDialog key={current.serial} target={current.target} onClose={useDuplicateHost.getState().close} /></Suspense>;
  const Component = current.target.kind === 'thread' ? ThreadDuplicate : DuplicateDialog;
  return <Suspense fallback={<Dialog open title={t('duplicate.title', { name: current.target.name })} onOpenChange={(open) => { if (!open) useDuplicateHost.getState().close(); }} footer={<Button onClick={useDuplicateHost.getState().close}>{t('common.cancel')}</Button>}><SkeletonRows rows={1} /></Dialog>}><Component key={current.serial} target={current.target} quick={current.quick} onClose={useDuplicateHost.getState().close} /></Suspense>;
}
