/** Captured summary facts keep their producer, scope and units distinct from current choices. */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { RequestTokens, SummaryProducerView, SummaryResultFacts } from '@/generated/api-types';
import { modelResource } from '@/features/resource-facts/facts';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { SummaryProducer } from './model';
import { formatNumber } from '@/lib/format';
import type { DisclosureFact } from '@/ui/FactDisclosure';

export function summaryProducer(view: SummaryProducerView | null | undefined, fallback: string): SummaryProducer | null {
  if (view === null || view === undefined) return null;
  if (view.offering !== null) {
    const model = { model_name: view.offering, display_name: view.title, parameters: {} };
    return view.provider === null ? { kind: 'model', model, providerId: null } : modelResource(model, view.provider.id);
  }
  return { kind: 'extension', name: view.kind === 'model' ? fallback : view.title };
}

export function ProducerFact({ view }: { view: SummaryProducerView | null | undefined }) {
  const { t } = useTranslation();
  if (view === null || view === undefined || view.kind === 'model' && view.offering === null) return <>{t('factDisclosure.notReported')}</>;
  if (view.kind === 'pool') return <span>{t('pools.selectedModel')}</span>;
  if (view.offering === null) return <span>{view.title}</span>;
  const model = { model_name: view.offering, display_name: view.title, parameters: {} };
  return <ResourceChip hint={false} subject={view.provider === null ? { kind: 'model', model, providerId: null } : modelResource(model, view.provider.id)} />;
}

export function tokenWords(value: number | null | undefined, t: TFunction, locale: string): string | undefined {
  return value === null || value === undefined ? undefined : t('summaryFacts.tokens', { count: value, number: formatNumber(value, locale) });
}

/** No missing dimension is manufactured from legacy zero-filled token totals. */
export function usageWords(usage: RequestTokens | null | undefined, t: TFunction, locale: string): string | undefined {
  if (usage === null || usage === undefined) return undefined;
  const main = [
    [t('summaryBox.rateInput'), usage.input_tokens], [t('summaryBox.rateOutput'), usage.output_tokens], [t('summaryFacts.total'), usage.total_tokens],
  ] as const;
  const extra = [
    [t('summaryBox.rateCachedInput'), usage.cached_input_tokens], [t('summaryBox.rateCacheWrite'), usage.cache_write_tokens], [t('summaryFacts.reasoning'), usage.reasoning_tokens],
  ] as const;
  const parts = [...main, ...extra.filter(([, value]) => typeof value === 'number' && value > 0)]
    .flatMap(([name, value]) => value === null ? [] : [`${name} ${tokenWords(value, t, locale)}`]);
  return parts.length === 0 ? undefined : parts.join(' · ');
}

export function conformanceFragment(facts: SummaryResultFacts | null | undefined, t: TFunction): string | undefined {
  if (facts?.rejected_whole === true) return t('summaryFacts.answerDiscarded');
  if (facts?.removed_characters !== null && facts?.removed_characters !== undefined && facts.removed_characters > 0) return t('summaryFacts.shortened');
  if (facts?.flattened === true || facts?.first_line_only === true || (facts?.unknown_tags ?? 0) > 0 || (facts?.duplicate_tags ?? 0) > 0 || (facts?.over_budget_tags ?? 0) > 0) return t('summaryFacts.adjusted');
  return undefined;
}

export function conformanceFacts(facts: SummaryResultFacts | null | undefined, t: TFunction, locale: string): DisclosureFact[] {
  const changes: string[] = [];
  if (facts?.flattened === true) changes.push(t('summaryFacts.whitespace'));
  if (facts?.first_line_only === true) changes.push(t('summaryFacts.firstLine'));
  for (const [key, count] of [
    ['summaryFacts.charactersRemoved', facts?.removed_characters], ['summaryFacts.unknownTags', facts?.unknown_tags],
    ['summaryFacts.duplicateTags', facts?.duplicate_tags], ['summaryFacts.overBudgetTags', facts?.over_budget_tags],
  ] as const) if (count !== null && count !== undefined && count > 0) changes.push(t(key, { count, number: formatNumber(count, locale) }));
  if (facts?.rejected_whole === true) changes.push(t('summaryFacts.wholeHandoff'));
  const captured = facts !== null && facts !== undefined && [facts.flattened, facts.first_line_only, facts.removed_characters, facts.unknown_tags, facts.duplicate_tags, facts.over_budget_tags, facts.rejected_whole].every((value) => value !== null);
  const output: ReactNode = changes.length === 0 ? undefined : <span className="flex min-w-0 flex-col gap-1">{changes.map((change) => <span key={change}>{change}</span>)}</span>;
  return [
    ...(changes.length > 0 || !captured ? [{ key: 'output', label: t('summaryFacts.outputChanges'), value: output }] : []),
    { key: 'usage', label: t('summaryFacts.recordedUsage'), value: usageWords(facts?.captured_usage, t, locale) },
  ];
}
