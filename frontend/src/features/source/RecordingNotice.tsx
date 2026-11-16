/** Whole recording has one short explanation and one door to its setting. */
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { indexingProgress } from '@/features/files/indexWords';
import { record } from '@/features/resources/UnitCommands';

export function recordingReason(facts: unknown): 'too_large' | 'live_tree' | null {
  const recording = record(record(facts).recording);
  return recording.state === 'unavailable' && (recording.reason === 'too_large' || recording.reason === 'live_tree') ? recording.reason : null;
}

export function recordingIndexingWords(t: TFunction, indexing: unknown): string {
  const progress = indexingProgress({ indexing });
  return progress === null ? t('largeTree.recordingIndexingUnknown')
    : t('largeTree.recordingIndexing', { files: new Intl.NumberFormat().format(progress.total), percent: progress.percent });
}

export function RecordingNotice({ environmentId, reason, indexing }: { environmentId: string | null | undefined; reason: string | null | undefined; indexing?: unknown }) {
  const { t } = useTranslation();
  if (reason !== 'too_large' && reason !== 'live_tree' && reason !== 'indexing') return null;
  const setting = reason === 'indexing' ? 'workspace.record_wait_secs' : 'workspace.record_max_bytes';
  return <p className="text-xs text-text-muted" data-recording-notice>
    {reason === 'indexing' ? recordingIndexingWords(t, indexing) : t(reason === 'too_large' ? 'largeTree.recordingTooLarge' : 'largeTree.recordingLive')}
    {environmentId !== null && environmentId !== undefined && <> <a className="rounded-sm text-accent-text underline underline-offset-2" href={`/resources/environments/${encodeURIComponent(environmentId)}?tab=settings&setting=${setting}`}>{t(reason === 'indexing' ? 'largeTree.recordingWait' : 'largeTree.recordingSetting')}</a></>}
  </p>;
}
