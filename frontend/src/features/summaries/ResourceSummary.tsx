/** Ordinary native read, explicit paid receipt, and the same root-pinned editable Ask draft. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { refreshResourceSummary, resourceSummaryQuery, summaryReaderKey, type ResourceSummaryRange, type ResourceSummaryTarget } from '@/api/summaries';
import { openAskWithDraft } from '@/features/ask/open';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { programBackend } from '@/features/settings/capabilityBackendValue';
import { CapabilityHelper } from '@/features/timeline/tools/CapabilityHelper';
import { CapabilityPlan } from '@/features/timeline/tools/CapabilityPlan';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { SummaryBox } from './SummaryBox';
import { summaryActionKey } from './model';
import { conformanceFacts, conformanceFragment, ProducerFact, summaryProducer, tokenWords, usageWords } from './facts';

export interface ResourceSummaryProps {
  target: ResourceSummaryTarget;
  reference: string;
  range?: ResourceSummaryRange;
  enabled?: boolean;
  onConfigure?: () => void;
  className?: string;
  /** Shown as one closed row while it has nothing yet (a place's page). */
  foldWhenEmpty?: boolean;
}
export function ResourceSummary(props: ResourceSummaryProps) {
  const reader = useStore(client.session, summaryReaderKey);
  return <CurrentSummary key={JSON.stringify([reader, props.target.kind, props.target.id, props.range?.after ?? null, props.range?.through ?? null])} {...props} reader={reader} />;
}
function CurrentSummary({ target, reference, range, enabled = true, onConfigure, className, foldWhenEmpty = false, reader }: ResourceSummaryProps & { reader: string }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? 'en';
  const cache = useQueryClient();
  const actKey = useActKey();
  const options = resourceSummaryQuery(target, range);
  const query = useQuery({ ...options, enabled });
  if (!enabled) return null;
  if (query.isError) return <ErrorAlert error={query.error} action={<Button size="sm" onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />;
  if (query.data === undefined) return <SkeletonRows rows={foldWhenEmpty ? 1 : 2} />;
  const reply = query.data;
  const manualReason = reply.manual_problem === 'cap_reached' ? t('summaryBox.manualCap')
    : reply.manual_problem === 'no_model' ? t('summaryBox.manualModel') : undefined;
  const refresh = async () => {
    if (reader !== summaryReaderKey() || reply.inspecting) return;
    await refreshResourceSummary(target, actKey([target.kind, target.id, range?.after ?? null, range?.through ?? null]), range);
    if (reader !== summaryReaderKey()) return;
    // Acknowledgement ends the paid act. A failed readback retries only the ordinary read.
    actKey.reset();
    await cache.query(options);
  };
  const rates = reply.route?.prices ?? {};
  const rateRows = [
    ['input_per_mtok', t('summaryBox.rateInput')], ['cached_input_per_mtok', t('summaryBox.rateCachedInput')],
    ['cache_write_per_mtok', t('summaryBox.rateCacheWrite')], ['output_per_mtok', t('summaryBox.rateOutput')],
  ] as const;
  const readableRates = rateRows.flatMap(([key, label]) => {
    const rate = rates[key];
    return rate?.currency === 'credits' && rate.unit === 'million_tokens' ? [{ key, label, amount: rate.amount }] : [];
  });
  const program = reply.program;
  const retained = program !== undefined && program !== null;
  const native = retained || programBackend({route:reply.route?.route_view?.id});
  const subscription = retained ? program.provider?.id : native ? reply.route?.provider?.id : undefined;
  const proposal = reply.inspecting ? null : reply.refresh_proposal;
  const lastAttempt = reply.last_attempt;
  const groups: FactGroup[] = [
    ...(reply.text === null ? [] : [{ key: 'last-result', kind: 'last_result' as const, facts: [
      { key: 'producer', label: t('summaryFacts.producer'), value: <ProducerFact view={reply.producer} /> },
      { key: 'updated', label: t('summaryFacts.lastUpdated'), value: reply.updated_at === null ? undefined : <RelativeTime at={reply.updated_at} /> },
      ...conformanceFacts(reply.result_facts, t, locale),
    ] }]),
    ...(proposal === null || proposal === undefined ? [] : [{ key: 'next-refresh', kind: 'next_refresh' as const, facts: [
      { key: 'producer', label: t('summaryFacts.producer'), value: <ProducerFact view={proposal.producer} /> },
      { key: 'payer', label: t('summaryFacts.usageOwner'), value: proposal.usage_owner === null ? t('summaryFacts.notShared') : <UserChip user={proposal.usage_owner} /> },
      { key: 'input', label: t('summaryFacts.inputEstimate'), value: proposal.input_estimate_tokens === null ? undefined : `${proposal.estimate_basis === 'assumed' ? '~' : ''}${tokenWords(proposal.input_estimate_tokens, t, locale)}` },
      { key: 'output', label: t('summaryFacts.outputAllowance'), value: tokenWords(proposal.output_allowance_tokens, t, locale) },
      ...(manualReason === undefined ? [] : [{ key: 'reason', label: t('common.reason'), value: manualReason }]),
    ] }]),
    ...(reply.state !== 'no_summarizer' || proposal !== null && proposal !== undefined ? [] : [{ key: 'refresh-unavailable', kind: 'next_refresh' as const, facts: [
      { key: 'producer', label: t('summaryFacts.producer'), value: t('summaryBox.state.no_summarizer') },
      { key: 'reason', label: t('common.reason'), value: manualReason },
    ] }]),
    ...(lastAttempt === null || lastAttempt === undefined || lastAttempt.outcome === 'published' ? [] : [{ key: 'attempt', kind: 'recorded_outcome' as const, facts: [
      { key: 'outcome', label: t('summaryFacts.outcome'), value: t(`summaryFacts.outcomes.${lastAttempt.outcome}`) },
      ...(lastAttempt.outcome !== 'discarded' ? [] : [{ key: 'reason', label: t('summaryFacts.discarded'), value: lastAttempt.discard_reason === null ? undefined : t(`summaryFacts.discard.${lastAttempt.discard_reason}`) }]),
      { key: 'producer', label: t('summaryFacts.producer'), value: <ProducerFact view={lastAttempt.producer} /> },
      { key: 'payer', label: t('summaryFacts.usageOwner'), value: lastAttempt.usage_owner === null ? t('summaryFacts.notShared') : <UserChip user={lastAttempt.usage_owner} /> },
      ...conformanceFacts(lastAttempt.result_facts, t, locale).filter((fact) => fact.key !== 'usage'),
      { key: 'usage', label: t('summaryFacts.recordedUsage'), value: usageWords(lastAttempt.captured_usage, t, locale) },
      { key: 'at', label: t('summaryFacts.lastAttempt'), value: <RelativeTime at={lastAttempt.at} /> },
    ] }]),
    ...(native ? [] : [{ key: 'rates', title: t('summaryBox.baseRates'), facts: readableRates.map((rate) => ({ key: rate.key, label: rate.label, value: t('summaryBox.perMillion', { amount: rate.amount }) })) }]),
  ];
  const fragment = lastAttempt?.outcome === 'discarded' ? t('summaryFacts.resultDiscarded') : conformanceFragment(reply.result_facts, t);
  const detailsLabel = t(summaryActionKey(target.kind, 'details'));
  const details = <>
    {proposal !== null && proposal !== undefined && <span className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-1 text-sm text-text-muted" data-summary-proposal>{t('summaryFacts.nextRefresh')}<ProducerFact view={proposal.producer} /></span>}
    <FactDisclosure label={detailsLabel} groups={groups} fragment={fragment}
      header={<div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <p className="text-sm font-medium text-text">{detailsLabel}</p>
        <ArticleGuideLink article="summaries" title={t('summaryBox.title')} />
      </div>}>
      {reply.input !== null && <p className="text-sm text-text-muted" data-summary-input-facts>{t('summaryBox.inputFacts', { included: reply.input.included_items, shortened: reply.input.shortened_items, omitted: reply.input.omitted_items })}</p>}
      {program?.helper_id !== undefined && program.helper_id !== null ? <CapabilityHelper result={{helper:{thread_id:program.helper_id,actor:program.actor,provider_id:subscription}}} />
        : subscription !== undefined && <ResourceDetails label={t('timeline.capabilitySubscription')}>{()=> <CapabilityPlan providerId={subscription} />}</ResourceDetails>}
      {native && subscription === undefined && <StateLine compact title={t('providerPlan.roomHidden')} />}
      {proposal?.producer.kind !== 'pool' && !native && readableRates.length === 0 && <div className="flex min-w-0 flex-col gap-2"><p className="text-sm font-medium text-text">{t('summaryBox.baseRates')}</p><StateLine compact title={t('summaryBox.noRates')} /></div>}
    </FactDisclosure>
  </>;
  return <SummaryBox readerKey={reader} target={{ kind: target.kind, reference }} className={className} foldWhenEmpty={foldWhenEmpty}
    summary={{ revision: JSON.stringify(reply), state: reply.state, text: reply.text, producer: summaryProducer(reply.producer, t('summaryBox.model')), updated_at: reply.updated_at, wait: reply.wait,
      ...(reply.problem === null ? {} : { error: new ApiError(reply.problem, null) }) }}
    onRefresh={refresh} refreshBlockedReason={reply.inspecting ? t('summaryBox.inspection') : manualReason}
    nothingNew={target.kind === 'chat' && range !== undefined && range.after >= range.through}
    askBlockedReason={reply.inspecting ? t('summaryBox.inspection') : undefined} {...(onConfigure === undefined ? {} : { onConfigure })} details={details}
    onAsk={({ summary }) => { if (reader !== summaryReaderKey() || reply.inspecting) return; openAskWithDraft({ root: reply.root_org_id,
      text: [t('summaryBox.ask'), reference, ...(summary === null ? [] : summary.split('\n').map((line) => `> ${line}`))].join('\n\n') }); }} />;
}
