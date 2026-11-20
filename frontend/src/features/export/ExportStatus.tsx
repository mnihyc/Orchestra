/** The archives being written, among the jobs of the status bar: what runs and how far, and what has just ended; pressing it opens the list of one's exports.
 *
 * It says nothing while no archive is being written and none has just ended. One running archive is named with its
 * percentage; several are counted. A ready archive and a failed one stay until the person has looked at the list.
 */
import { useNavigate } from '@tanstack/react-router';
import { CircleAlert, Download, LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { ActionIcon, actionName } from '@/ui/actions';
import { StatusPart } from '@/ui/StatusBar';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

import { dismissFinished, useWatched } from './jobs';

export function ExportStatus() {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const navigate = useNavigate();
  const items = useWatched((state) => state.items);
  if (items.length === 0) return null;
  const running = items.filter((one) => one.state === 'running');
  const ready = items.filter((one) => one.state === 'ready');
  const partial = ready.filter((one) => one.omitted !== null && one.omitted > 0);
  const failed = items.filter((one) => one.state === 'failed');
  const open = () => {
    dismissFinished();
    void navigate({ to: '/settings/exports' });
  };
  return (
    <>
      {running.length > 0 && (
        <StatusPart
          icon={<LoaderCircle size={16} className="animate-spin" aria-hidden />}
          hint={t('export.status.runningHint')}
        >
          {running.length === 1 && running[0] !== undefined
            ? t('export.status.running', { place: running[0].place, percent: running[0].percent })
            : t('export.status.runningMany', { count: running.length })}
        </StatusPart>
      )}
      {ready.length > 0 && (
        <StatusPart icon={<Download size={16} aria-hidden />} hint={t('export.status.readyHint')}>
          {ready.length === 1 && ready[0] !== undefined
            ? ready[0].omitted === null ? t('export.status.readyUnknown', { place: ready[0].place }) : ready[0].omitted > 0 ? t('export.status.partial', { place: ready[0].place, number: formatNumber(ready[0].omitted, locale) }) : t('export.status.ready', { place: ready[0].place })
            : partial.length > 0 ? t('export.status.readyWithPartial', { number: formatNumber(ready.length, locale) }) : ready.some((one) => one.omitted === null) ? t('export.status.readyWithUnknown', { number: formatNumber(ready.length, locale) }) : t('export.status.readyMany', { count: ready.length })}
        </StatusPart>
      )}
      {failed.length > 0 && (
        <StatusPart tone="danger" icon={<CircleAlert size={16} aria-hidden />} hint={t('export.status.failedHint')}>
          {t('export.status.failed', { count: failed.length })}
        </StatusPart>
      )}
      <StatusPart icon={<ActionIcon action="yourExports" className="size-4" />} onOpen={open}>{actionName('yourExports')}</StatusPart>
    </>
  );
}
