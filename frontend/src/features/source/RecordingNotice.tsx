/** Captured recording facts use one disclosure; routine preparation stays in message actions. */
import { PathLink } from '@/shell/TrailLink';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { indexingProgress } from '@/features/files/indexWords';
import { record } from '@/features/resources/UnitCommands';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure } from '@/ui/FactDisclosure';

export function recordingReason(facts: unknown): 'too_large' | 'live_tree' | null {
  const recording = record(record(facts).recording);
  return recording.state === 'unavailable' && (recording.reason === 'too_large' || recording.reason === 'live_tree') ? recording.reason : null;
}

export function recordingIndexingWords(t: TFunction, indexing: unknown, locale: string): string {
  const progress = indexingProgress({ indexing });
  return progress === null ? t('largeTree.recordingIndexingUnknown')
    : t(progress.percent === 100 ? 'largeTree.recordingPreparing' : 'largeTree.recordingIndexing', {
      files: formatNumber(progress.total, locale), percent: formatNumber(progress.percent, locale),
    });
}

export function RecordingNotice({ environmentId, reason, indexing, placement = 'notice' }: {
  environmentId: string | null | undefined; reason: string | null | undefined; indexing?: unknown; placement?: 'notice' | 'actions';
}) {
  const { t } = useTranslation();
  const format = useFormatContext();
  if (reason !== 'too_large' && reason !== 'live_tree' && reason !== 'indexing') return null;
  if ((reason === 'indexing') !== (placement === 'actions')) return null;
  const setting = reason === 'indexing' ? 'workspace.record_wait_secs' : 'workspace.record_max_bytes';
  const disclosure = <FactDisclosure label={t('largeTree.recordingDetails')}
    fragment={reason === 'indexing' ? undefined : t('largeTree.recordingUnavailable')}
    groups={[{ key: 'recording', title: reason === 'indexing' ? t('largeTree.recordingAtStart') : undefined, facts: [
      { key: 'coverage', label: t('largeTree.wholeRecording'), value: reason === 'indexing'
        ? recordingIndexingWords(t, indexing, format.locale) : t(reason === 'too_large' ? 'largeTree.recordingTooLarge' : 'largeTree.recordingLive') },
      { key: 'recovery', label: t('largeTree.recoverableFiles'), value: t('largeTree.recordingFileTools') },
      ...(environmentId !== null && environmentId !== undefined ? [{ key: 'setting', label: t('largeTree.recordingPolicy'), value:
        <PathLink className="rounded-sm text-accent-text underline underline-offset-2" href={`/resources/environments/${encodeURIComponent(environmentId)}?tab=settings&setting=${setting}`}>
          {t(reason === 'indexing' ? 'largeTree.recordingWait' : 'largeTree.recordingSetting')}
        </PathLink> }] : []),
    ] }]} />;
  return placement === 'actions' ? disclosure : <p className="text-sm text-text-muted" data-recording-notice>{disclosure}</p>;
}
