/** The one import dialog and its job watch live in the workbench frame. */
import { useNavigate } from '@tanstack/react-router';
import { lazy, Suspense, useCallback, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { closeImport, openImportJob, openImportReport, useImportHost } from '@/features/people/importHostState';
import { type WatchedImport, useImportJobWatch } from '@/features/people/importJobs';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';

const ImportDialog = lazy(() => import('./ImportDialog').then((module) => ({ default: module.ImportDialog })));

export function ImportHost() {
  const { t } = useTranslation();
  const state = useImportHost((value) => value);
  const user = useStore(client.session, (session) => session.userId);
  const reader = useStore(client.session, (session) => session.readerVersion ?? 0);
  const navigate = useNavigate();
  const open = useCallback((item: WatchedImport) => {
    if (item.state === 'running') openImportJob(item.job, item.place);
    else if (item.report !== null) openImportReport(item.report);
    else void navigate({ to: '/settings/imports' });
  }, [navigate]);
  useImportJobWatch(open);
  useEffect(() => {
    const held = useImportHost.getState();
    if (held.target !== null && (held.user !== user || held.reader !== reader)) closeImport();
  }, [user, reader]);
  if (state.target === null || user === null || state.user !== user || state.reader !== reader) return null;
  const key = state.target.kind === 'new' ? `new:${state.target.place?.id ?? ''}:${state.target.folder?.id ?? ''}`
    : state.target.kind === 'job' ? `job:${state.target.job}` : `report:${state.target.report}`;
  return <Suspense fallback={<Dialog open onOpenChange={(open) => { if (!open) closeImport(); }} title={t(state.target.kind === 'report' ? 'importWizard.report.title' : 'importWizard.dialog.title')} size="lg"
    footer={<Button onClick={closeImport}>{t('common.close')}</Button>}><SkeletonRows rows={4} /></Dialog>}><ImportDialog key={key} target={state.target} onClose={closeImport} /></Suspense>;
}
