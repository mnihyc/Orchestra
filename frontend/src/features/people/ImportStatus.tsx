/** A watched import in the status bar opens its progress or retained report. */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { CircleAlert, CircleCheck, LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { openImportJob, openImportReport } from '@/features/people/importHostState';
import { dismissImport, type WatchedImport, useWatchedImports } from '@/features/people/importJobs';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { StatusPart } from '@/ui/StatusBar';

function ImportStatusItem({ item }: { item: WatchedImport }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const navigate = useNavigate();
  const place = useQuery({ ...nodeQuery(item.place ?? ''), enabled: item.place !== null });
  const placeName = place.isError ? t('importWizard.job.unknownPlace') : place.data?.name ?? t('importWizard.job.unknownPlace');
  const title = item.reconnecting ? t('importWizard.job.reconnecting') : item.state === 'running'
    ? item.undo ? t('importWizard.job.undoing', { percent: formatNumber(item.percent, locale) }) : t('importWizard.job.running', { place: placeName, percent: formatNumber(item.percent, locale) })
    : item.undo && item.state === 'done' ? t('importWizard.job.undone') : t(item.state === 'done' ? 'importWizard.job.finished' : item.state === 'failed' ? 'importWizard.job.failed' : 'importWizard.job.cancelled');
  const open = () => {
    if (item.state === 'running') openImportJob(item.job, item.place);
    else {
      dismissImport(item.job);
      if (item.report !== null) openImportReport(item.report);
      else void navigate({ to: '/settings/imports' });
    }
  };
  const Icon = item.state === 'running' ? LoaderCircle : item.state === 'failed' ? CircleAlert : CircleCheck;
  return <StatusPart icon={<Icon size={16} className={item.state === 'running' ? 'animate-spin' : undefined} aria-hidden />} tone={item.state === 'failed' ? 'danger' : 'plain'} onOpen={open}>{title}</StatusPart>;
}

export function ImportStatus() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const navigate = useNavigate();
  const watched = useWatchedImports((state) => state);
  const user = useStore(client.session, (state) => state.userId);
  if (user === null || watched.user !== user || watched.items.length === 0) return null;
  const first = watched.items[0];
  return <span className="flex min-w-0 max-w-48 items-stretch">
    {watched.items.length > 1 ? <StatusPart icon={<CircleCheck size={16} aria-hidden />} onOpen={() => void navigate({ to: '/settings/imports' })}>
      {t('importWizard.job.many', { number: formatNumber(watched.items.length, locale) })}
    </StatusPart> : first !== undefined && <ImportStatusItem item={first} />}
  </span>;
}
