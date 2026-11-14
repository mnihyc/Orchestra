/** The same retained request facts in compact rows and the record's details. */
import { useTranslation } from 'react-i18next';
import type { UsageEvent } from '@/generated/api-types';
import { Facts } from '@/features/resources/Facts';
import { CopyId } from '@/ui/CopyId';
import { useFormatContext } from '@/lib/appearance';
import {purposeLabel} from './words';
import {serviceQuantityWords,serviceQuoteWords} from './services';
import { requestInputParts, requestSpeed, speedText, timeText } from './performance';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
/** Turn aggregates retain measured units without claiming another native model request. */
export function nativeCounterRows(event: Pick<UsageEvent, 'scope' | 'origin' | 'timing'>): { capability: string; quantity: number | null }[] {
  if (event.scope !== 'turn' || event.origin !== 'harness' || !record(event.timing) || !record(event.timing.declared_counters)) return [];
  return Object.entries(event.timing.declared_counters).flatMap(([capability, measurement]) => {
    if (!record(measurement)) return [];
    if (measurement.state === 'unread') return [{ capability, quantity: null }];
    return measurement.state === 'told' && typeof measurement.value === 'number' && Number.isInteger(measurement.value) && measurement.value >= 0
      ? [{ capability, quantity: Number.isSafeInteger(measurement.value) ? measurement.value : null }] : [];
  });
}

export function TokenFacts({ event }: { event: UsageEvent }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if(event.timing?.service_usage!==undefined && event.timing.service_usage!==null) return null;
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
  const counts = nativeCounterRows(event);
  const names: Record<string, string> = {
    generation: t('capabilityRoutes.names.generation'),
    image_input: t('capabilityRoutes.names.image_input'),
    web_search: t('capabilityRoutes.names.web_search'),
    page_fetch: t('capabilityRoutes.names.page_fetch'),
    image_generation: t('capabilityRoutes.names.image_generation'),
    embeddings: t('capabilityRoutes.names.embeddings'),
    provider_facts: t('capabilityRoutes.names.provider_facts'),
    summary: t('capabilityRoutes.names.summary'),
  };
  const reported = timing?.reported_cost;
  const vendorTotal = reported?.state === 'told' ? `${reported.value.amount} ${reported.value.unit ?? t('usageRequest.unknownUnit')}` : reported?.state === 'unread' || reported?.state === 'empty' ? t('usageRequest.costNotReported') : null;
  const code = timing?.outcome.state === 'told' ? timing.outcome.value : null;
  const reason = timing?.error_reason;
  const reasonText = reason === undefined || reason === null ? null : ['cyber','safety','other','access','credit_program'].includes(reason)
    ? t(`usageRequest.reasons.${reason}` as 'usageRequest.reasons.other') : reason;
  const number = (value: number | null | undefined) => value === null || value === undefined ? t('usagePerf.dash') : new Intl.NumberFormat(locale).format(value);
  const ownerName = event.thread_owner?.display_name;
  const service=timing?.service_usage;
  const rows = [
    ...(service===undefined || service===null?[]:[{name:t('usage.chip.purpose'),value:purposeLabel(event.purpose,t)},
      {name:t('usage.serviceUnits.title'),value:serviceQuantityWords(service.unit,service.quantity.state==='told'?service.quantity.value:null,t,locale)},
      ...(!details?[]:[{name:t('usage.serviceUnits.price'),value:serviceQuoteWords(service,t)},
        ...(service.credits_per_unit===undefined || service.credits_per_unit===null?[]:[{name:t('usage.serviceUnits.applied'),value:service.credits_per_unit}])])]),
    ...(counts.length === 0 ? [] : [{ name: t('usageRequest.measurement'), value: t('usageRequest.nativeAggregate') }]),
    ...counts.map(({ capability, quantity }) => {
      const name = Object.hasOwn(names, capability) ? names[capability] : undefined;
      return { name: typeof name === 'string' ? name : capability, value: number(quantity) };
    }),
    ...(event.running ? [{ name: t('usagePerf.outcome'), value: t('usagePerf.running') }] : code === null ? [] : [{ name: t('usagePerf.outcome'), value: t(`usagePerf.outcomes.${code}` as 'usagePerf.outcomes.done', { defaultValue: t('usagePerf.outcomes.harness_error') }) }]),
    ...(reasonText === null ? [] : [{ name: t('usageRequest.reason'), value: reasonText }]),
    ...(timing?.upstream_status === undefined || timing.upstream_status === null ? [] : [{ name: t('usageRequest.status'), value: number(timing.upstream_status) }]),
    ...(event.scope !== 'request' || timing?.attempt === undefined || timing.attempt === null ? [] : [{ name: t('usageRequest.attempt'), value: number(timing.attempt) }]),
    ...(!details && event.tool_calls === undefined && event.tool_ms === undefined ? [] : [
      { name: t('usageRequest.tools'), value: number(event.tool_calls) },
      { name: t('usageRequest.toolTime'), value: timeText(event.tool_ms, locale, t) },
    ]),
    ...(!speed && !details ? [] : [{ name: t('usagePerf.requestSpeed'), value: speedText(requestSpeed(event.tokens.output_tokens,timing), locale, t) }]),
    ...(!details || vendorTotal === null ? [] : [
      { name: t('usageRequest.vendorTotal'), value: vendorTotal },
      { name: t('usageRequest.countedCredits'), value: event.credits ?? t('usagePerf.dash') },
    ]),
    ...(!details ? [] : [
      { name: t('usageRequest.providerId'), value: timing?.provider_request_id === undefined || timing.provider_request_id === null ? t('usagePerf.dash') : <CopyId value={timing.provider_request_id} /> },
      { name: t('usageRequest.adapterId'), value: event.request_ref === undefined || event.request_ref === null ? t('usagePerf.dash') : <CopyId value={event.request_ref} /> },
      { name: t('usageRequest.owner'), value: ownerName === '' ? t('usagePerf.dash') : ownerName ?? t('usagePerf.dash') },
    ]),
  ];
  return rows.length === 0 ? null : <Facts compact rows={rows} />;
}
