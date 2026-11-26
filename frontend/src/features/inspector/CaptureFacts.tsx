/** One native source's recorded capture boundary, kept distinct from its current retention setting. */
import { useTranslation } from 'react-i18next';
import type { RawCapture, RawCaptureOmissionCause, RawRetention, SettingSource } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { FactDisclosure, type DisclosureFact } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

const REASONS: Record<RawCaptureOmissionCause, string> = {
  capture_off: 'threadViews.diagnostics.facts.captureOff',
  masking_unavailable: 'threadViews.diagnostics.facts.maskingUnavailable',
  capture_allowance: 'threadViews.diagnostics.facts.allowanceReached',
  retention_elapsed: 'threadViews.diagnostics.facts.retentionElapsed',
};

export function CaptureFacts({ capture = null, retention = null }: { capture?: RawCapture | null; retention?: RawRetention | null }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const number = (value: number) => formatNumber(value, locale);
  const range = (from: number, through: number) => `${number(from)}–${number(through)}`;
  const ranges = capture === null ? null : capture.retained_ranges.length === 0 ? t('threadViews.diagnostics.facts.none')
    : capture.retained_ranges.map((part) => range(part.from, part.through)).join(' · ');
  const omitted = capture?.omission_counts_by_cause.filter((entry) => entry.count > 0) ?? [];
  const incomplete = omitted.length > 0 || (capture?.truncated_notes ?? 0) > 0;
  const source = (key: string, from: SettingSource) => from.node?.redacted === true || from.node?.name === null
    || from.node === null && from.scope !== 'default' && from.scope !== 'system'
    ? <span>{t('threadViews.diagnostics.facts.notShared')}</span>
    : <SourceSettingLink source={from} def={{ key, class: 'policy' }} fallback={t('settings.editor.from.default')} />;
  const observed: DisclosureFact[] = [
    { key: 'retained', label: t('threadViews.diagnostics.facts.retained'), value: ranges },
    { key: 'observed', label: t('threadViews.diagnostics.facts.observedRange'), value: capture === null ? null : range(capture.observed_range.from, capture.observed_range.through) },
    ...(omitted.length === 0 ? [{ key: 'omitted', label: t('threadViews.diagnostics.facts.omissions'), value: capture === null ? null : number(0) }]
      : omitted.map((entry) => ({ key: `omitted-${entry.cause}`, label: t('threadViews.diagnostics.facts.omitted', { count: entry.count, number: number(entry.count) }), value: t(REASONS[entry.cause]) }))),
    { key: 'oversized', label: t('threadViews.diagnostics.facts.oversized'), value: capture === null ? null : number(capture.oversize_notes) },
    { key: 'shortened', label: t('threadViews.diagnostics.facts.shortened'), value: capture === null ? null : number(capture.truncated_notes) },
    ...(capture?.earlier_unreported === true ? [{ key: 'earlier', label: t('threadViews.diagnostics.facts.earlier'), value: null }] : []),
    { key: 'captured', label: t('threadViews.diagnostics.facts.observed'), value: capture === null ? null : <RelativeTime at={capture.captured_at} /> },
    { key: 'decision', label: t('threadViews.diagnostics.facts.decision'), value: capture === null ? null : <span className="inline-flex flex-wrap gap-1">
      {t('threadViews.diagnostics.frame', { n: number(capture.limits.native_n) })} · <RelativeTime at={capture.limits.captured_at} />
    </span> },
    { key: 'frameBytes', label: t('threadViews.diagnostics.facts.frameBytes'), value: capture === null ? null
      : capture.limits.frame_bytes === 0 ? t('threadViews.diagnostics.facts.off') : formatBytes(capture.limits.frame_bytes, locale),
    source: capture === null ? null : source('diagnostics.frame_bytes', capture.limits.frame_source) },
    { key: 'headBytes', label: t('threadViews.diagnostics.facts.headBytes'), value: capture === null ? null
      : capture.limits.head_bytes === 0 ? t('threadViews.diagnostics.facts.off') : formatBytes(capture.limits.head_bytes, locale),
    source: capture === null ? null : source('diagnostics.frame_head_bytes', capture.limits.head_source) },
    { key: 'frames', label: t('threadViews.diagnostics.facts.frames'), value: capture === null ? null
      : capture.limits.frames_per_segment === 0 ? t('threadViews.diagnostics.facts.off') : number(capture.limits.frames_per_segment),
    source: capture === null ? null : source('diagnostics.frames_per_segment', capture.limits.frames_source) },
  ];
  return <FactDisclosure fragment={incomplete ? t('threadViews.diagnostics.facts.incomplete')
    : capture !== null && capture.retained_ranges.length > 0 ? t('threadViews.diagnostics.facts.retainedClosed', { range: ranges }) : undefined}
  groups={[
    { key: 'capture', kind: 'recorded_outcome', facts: observed },
    { key: 'retention', kind: 'current_setting', facts: [
      { key: 'duration', label: t('threadViews.diagnostics.facts.retention'), value: retention === null ? null : formatSpan(retention.days * 86400000, { locale, timeZone }),
        source: retention === null ? null : source('diagnostics.retention_days', retention.source) },
      { key: 'observed', label: t('threadViews.diagnostics.facts.observed'), value: retention === null ? null : <RelativeTime at={retention.measured_at} /> },
    ] },
  ]} />;
}
