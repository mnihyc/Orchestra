/** The same retained request facts in compact rows and the record's details. */
import { useTranslation } from 'react-i18next';
import type { UsageEvent } from '@/generated/api-types';
import { Facts } from '@/features/resources/Facts';
import { useFormatContext } from '@/lib/appearance';
import { requestInputParts, requestSpeed, speedText, timeText } from './performance';

export function TokenFacts({ event }: { event: UsageEvent }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const parts = requestInputParts(event);
  const number = (value: number | null) => value === null ? t('usagePerf.dash') : new Intl.NumberFormat(locale).format(value);
  return <Facts compact rows={[
    { name: t('usageRequest.uncached'), value: number(parts.uncached) },
    { name: t('usageRequest.cached'), value: number(parts.cached) },
    { name: t('usageRequest.cacheWrite'), value: number(parts.written) },
  ]} />;
}

export function RequestFacts({ event, details = false, speed = false }: { event: UsageEvent; details?: boolean; speed?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const timing = event.timing;
  const code = timing?.outcome.state === 'told' ? timing.outcome.value : null;
  const reason = timing?.error_reason;
  const reasonText = reason === undefined || reason === null ? null : ['cyber','safety','other','access'].includes(reason)
    ? t(`usageRequest.reasons.${reason}` as 'usageRequest.reasons.other') : reason;
  const number = (value: number | null | undefined) => value === null || value === undefined ? t('usagePerf.dash') : new Intl.NumberFormat(locale).format(value);
  const rows = [
    ...(event.running ? [{ name: t('usagePerf.outcome'), value: t('usagePerf.running') }] : code === null ? [] : [{ name: t('usagePerf.outcome'), value: t(`usagePerf.outcomes.${code}` as 'usagePerf.outcomes.done', { defaultValue: t('usagePerf.outcomes.harness_error') }) }]),
    ...(reasonText === null ? [] : [{ name: t('usageRequest.reason'), value: reasonText }]),
    ...(timing?.upstream_status === undefined || timing.upstream_status === null ? [] : [{ name: t('usageRequest.status'), value: number(timing.upstream_status) }]),
    ...(event.scope !== 'request' || timing?.attempt === undefined || timing.attempt === null ? [] : [{ name: t('usageRequest.attempt'), value: number(timing.attempt) }]),
    ...(!details && event.tool_calls === undefined && event.tool_ms === undefined ? [] : [
      { name: t('usageRequest.tools'), value: number(event.tool_calls) },
      { name: t('usageRequest.toolTime'), value: timeText(event.tool_ms, locale, t) },
    ]),
    ...(!speed && !details ? [] : [{ name: t('usagePerf.requestSpeed'), value: speedText(requestSpeed(event.tokens.output_tokens,timing), locale, t) }]),
    ...(!details ? [] : [
      { name: t('usageRequest.providerId'), value: timing?.provider_request_id ?? t('usagePerf.dash') },
      { name: t('usageRequest.adapterId'), value: event.request_ref ?? t('usagePerf.dash') },
      { name: t('usageRequest.owner'), value: event.thread_owner?.display_name || t('usagePerf.dash') },
    ]),
  ];
  return rows.length === 0 ? null : <Facts compact rows={rows} />;
}
