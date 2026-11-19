/** Ordinary native read, explicit paid receipt, and the same root-pinned editable Ask draft. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { refreshResourceSummary, resourceSummaryQuery, summaryReaderKey, type ResourceSummaryRange, type ResourceSummaryReply, type ResourceSummaryTarget } from '@/api/summaries';
import { openAskWithDraft } from '@/features/ask/open';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { modelResource } from '@/features/resource-facts/facts';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { programBackend } from '@/features/settings/capabilityBackendValue';
import { CapabilityHelper } from '@/features/timeline/tools/CapabilityHelper';
import { CapabilityPlan } from '@/features/timeline/tools/CapabilityPlan';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { SummaryBox } from './SummaryBox';
import { summaryActionKey, type SummaryProducer } from './model';

export interface ResourceSummaryProps {
  target: ResourceSummaryTarget;
  reference: string;
  range?: ResourceSummaryRange;
  enabled?: boolean;
  onConfigure?: () => void;
  className?: string;
}
function producer(reply: ResourceSummaryReply, fallback: string): SummaryProducer | null {
  const supplied = reply.producer;
  if (supplied === null) return null;
  if (supplied.provider !== null && supplied.offering !== null) return modelResource({ model_name: supplied.offering, display_name: supplied.offering, parameters: {} }, supplied.provider.id);
  return { kind: 'extension', name: supplied.kind === 'model' ? fallback : supplied.title };
}
export function ResourceSummary(props: ResourceSummaryProps) {
  const reader = useStore(client.session, summaryReaderKey);
  return <CurrentSummary key={JSON.stringify([reader, props.target.kind, props.target.id, props.range?.after ?? null, props.range?.through ?? null])} {...props} reader={reader} />;
}
function CurrentSummary({ target, reference, range, enabled = true, onConfigure, className, reader }: ResourceSummaryProps & { reader: string }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const actKey = useActKey();
  const options = resourceSummaryQuery(target, range);
  const query = useQuery({ ...options, enabled });
  if (!enabled) return null;
  if (query.isError) return <ErrorAlert error={query.error} action={<Button size="sm" onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />;
  if (query.data === undefined) return <SkeletonRows rows={2} />;
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
  const detailsLabel = t(summaryActionKey(target.kind, 'details'));
  const details = <FactDisclosure label={detailsLabel} groups={native ? [] : [{ key: 'rates', facts: readableRates.map((rate) => ({ key: rate.key, label: rate.label, value: t('summaryBox.perMillion', { amount: rate.amount }) })) }]}
    header={<div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
        <p className="text-sm font-medium text-text">{t(native ? 'summaryBox.model' : 'summaryBox.baseRates')}</p>
        <ArticleGuideLink article="summaries" title={t('summaryBox.title')} />
      </div>}>
      {reply.input !== null && <p className="text-sm text-text-muted" data-summary-input-facts>{t('summaryBox.inputFacts', { included: reply.input.included_items, shortened: reply.input.shortened_items, omitted: reply.input.omitted_items })}</p>}
      {program?.helper_id !== undefined && program.helper_id !== null ? <CapabilityHelper result={{helper:{thread_id:program.helper_id,actor:program.actor,provider_id:subscription}}} />
        : subscription !== undefined && <ResourceDetails label={t('timeline.capabilitySubscription')}>{()=> <CapabilityPlan providerId={subscription} />}</ResourceDetails>}
      {native && subscription === undefined && <StateLine compact title={t('providerPlan.roomHidden')} />}
      {!native && readableRates.length === 0 && <StateLine compact title={t('summaryBox.noRates')} />}
  </FactDisclosure>;
  return <SummaryBox readerKey={reader} target={{ kind: target.kind, reference }} className={className}
    summary={{ revision: JSON.stringify(reply), state: reply.state, text: reply.text, producer: producer(reply, t('summaryBox.model')), updated_at: reply.updated_at, wait: reply.wait,
      ...(reply.problem === null ? {} : { error: new ApiError(reply.problem, null) }) }}
    onRefresh={refresh} refreshBlockedReason={reply.inspecting ? t('summaryBox.inspection') : manualReason}
    nothingNew={target.kind === 'chat' && range !== undefined && range.after >= range.through}
    askBlockedReason={reply.inspecting ? t('summaryBox.inspection') : undefined} {...(onConfigure === undefined ? {} : { onConfigure })} details={details}
    onAsk={({ summary }) => { if (reader !== summaryReaderKey() || reply.inspecting) return; openAskWithDraft({ root: reply.root_org_id,
      text: [t('summaryBox.ask'), reference, ...(summary === null ? [] : summary.split('\n').map((line) => `> ${line}`))].join('\n\n') }); }} />;
}
