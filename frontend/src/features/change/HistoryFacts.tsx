/** Native ordinals and captured context stay separate from the transcript's coverage. */
import type { TFunction } from 'i18next';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { ForkPlanHistory, HistoryContextAllowance, HistoryCoverage, HistoryTurnRange } from '@/generated/api-types';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatTokens } from '@/lib/format';
import { FactDisclosureBody, type DisclosureFact, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

function rangeWords(range: HistoryTurnRange, locale: string): string {
  return range.from === range.through ? formatNumber(range.from, locale) : `${formatNumber(range.from, locale)}–${formatNumber(range.through, locale)}`;
}

function allowanceFacts(allowance: HistoryContextAllowance | null | undefined, t: TFunction, locale: string): DisclosureFact[] {
  const basis = allowance?.source;
  const inapplicable = basis === 'native_session' || basis === 'summary' || basis === 'summary_fallback' || basis === 'nothing';
  const scope = allowance?.setting_scope;
  const policySource: ReactNode = scope === 'system' || scope === 'default' ? <SourceSettingLink source={{ scope, node: null }}
    def={{ key: 'thread.switch_carry_share', class: 'policy' }} fallback={t('factDisclosure.notReported')} />
    : scope === 'org' ? t('placeFacts.policyOrganization') : scope === 'project' ? t('placeFacts.policyProject') : scope === 'topic' ? t('placeFacts.policyTopic')
      : scope === 'thread' ? t('placeFacts.policyThread') : null;
  return [
    { key: 'share', label: t('placeFacts.contextShare'), value: allowance?.share === null || allowance?.share === undefined ? inapplicable ? t('placeFacts.notApplicable') : null : formatNumber(allowance.share / 100, locale, { style: 'percent' }), source: policySource },
    { key: 'allowance', label: t('placeFacts.contextAllowance'), value: allowance?.tokens === null || allowance?.tokens === undefined ? inapplicable ? t('placeFacts.notApplicable') : null : t('placeFacts.tokenAllowance', { tokens: formatTokens(allowance.tokens, locale) }) },
    { key: 'basis', label: t('placeFacts.basis'), value: basis === undefined ? null : t(`placeFacts.allowanceSources.${basis}`) },
  ];
}

export function HistoryPlanFacts({ history }: { history: ForkPlanHistory }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return <FactDisclosureBody groups={[{ key: 'history-proposal', title: t('placeFacts.proposal'), facts: [
    { key: 'selected', label: t('placeFacts.selectedTurns'), value: history.selected_range === null || history.selected_range === undefined ? history.method === 'none' ? t('placeFacts.notApplicable') : null : rangeWords(history.selected_range, locale) },
    ...allowanceFacts(history.context_allowance, t, locale),
  ] }]} />;
}

export function historyResultGroup(coverage: HistoryCoverage | null | undefined, t: TFunction, locale: string): FactGroup {
  return { key: 'history-outcome', kind: 'recorded_outcome', facts: [
    { key: 'method', label: t('placeFacts.historyMethod'), value: coverage === null || coverage === undefined ? null : t(`change.history.how.${coverage.method}`) },
    { key: 'selected', label: t('placeFacts.selectedTurns'), value: coverage?.selected_range === null || coverage?.selected_range === undefined ? coverage === null || coverage === undefined ? null : t('placeFacts.notApplicable') : rangeWords(coverage.selected_range, locale) },
    { key: 'carried', label: t('placeFacts.carriedTurns'), value: coverage === null || coverage === undefined ? null : coverage.context_form === 'summary' ? t('placeFacts.derivedSummary') : coverage.actual_ranges.length === 0 ? t('change.history.how.none') : coverage.actual_ranges.map((range) => rangeWords(range, locale)).join(' · ') },
    ...(coverage?.partial_turn === null || coverage?.partial_turn === undefined ? [] : [{ key: 'partial', label: t('placeFacts.partialTurn'), value: formatNumber(coverage.partial_turn, locale) }]),
    ...(coverage?.omitted_prefix === null || coverage?.omitted_prefix === undefined ? [] : [{ key: 'omitted', label: t('placeFacts.omittedPrefix'), value: t('placeFacts.beforeTurn', { turn: formatNumber(coverage.omitted_prefix, locale) }) }]),
    { key: 'form', label: t('placeFacts.contextForm'), value: coverage === null || coverage === undefined ? null : t(`placeFacts.contextForms.${coverage.context_form}`) },
    { key: 'transcript', label: t('placeFacts.transcript'), value: coverage === null || coverage === undefined ? null : t(`placeFacts.transcriptCoverage.${coverage.transcript_coverage}`) },
    ...allowanceFacts(coverage?.context_allowance, t, locale),
    { key: 'captured', label: t('placeFacts.recorded'), value: coverage === null || coverage === undefined ? null : <RelativeTime at={coverage.at} /> },
  ] };
}
