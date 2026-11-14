/** Whole recording has one short explanation and one door to its setting. */
import { useTranslation } from 'react-i18next';
import { record } from '@/features/resources/UnitCommands';

export function recordingReason(facts: unknown): 'too_large' | 'live_tree' | null {
  const recording = record(record(facts).recording);
  return recording.state === 'unavailable' && (recording.reason === 'too_large' || recording.reason === 'live_tree') ? recording.reason : null;
}

export function RecordingNotice({ environmentId, reason }: { environmentId: string | null | undefined; reason: string | null | undefined }) {
  const { t } = useTranslation();
  if (reason !== 'too_large' && reason !== 'live_tree') return null;
  return <p className="text-xs text-text-muted" data-recording-notice>
    {t(reason === 'too_large' ? 'largeTree.recordingTooLarge' : 'largeTree.recordingLive')}
    {environmentId !== null && environmentId !== undefined && <> <a className="rounded-sm text-accent-text underline underline-offset-2" href={`/resources/environments/${encodeURIComponent(environmentId)}?tab=settings&setting=workspace.record_max_bytes`}>{t('largeTree.recordingSetting')}</a></>}
  </p>;
}
