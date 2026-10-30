/** Profile arrangements name executors from their declarations, with bounded detail rows. */
import { useQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { executorCatalogueQuery } from '@/api/executors';
import type { ArrangementOffer } from '@/features/resources/agents/descriptor';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import { declaredExecutorNames } from '@/features/resources/executorNames';
import { arrangementName } from '@/features/resources/providers/Arrangements';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { Section } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { HelpHint } from '@/ui/HelpHint';

function BindingRows({ offers, declared }: { offers: readonly ArrangementOffer[]; declared: ReturnType<typeof declaredExecutorNames> }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const needsCatalogue = offers.some((offer) => offer.executor !== null && !declared.has(offer.executor));
  const catalogue = useQuery({ ...executorCatalogueQuery, enabled: needsCatalogue });
  const known = useMemo(() => declaredExecutorNames({ executors: catalogue.data }, locale), [catalogue.data, locale]);
  return <>
    {needsCatalogue && catalogue.isPending && <StateLine compact message={t('agents.overview.loadingExecutors')} />}
    {needsCatalogue && catalogue.isError && <ErrorState compact error={catalogue.error} message={t('machines.policy.executorReadFailed')} retrying={catalogue.isRefetching} onRetry={() => void catalogue.refetch()} />}
    <ul aria-label={t('agents.overview.bindings')} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
      {offers.map((offer) => {
        const executor = offer.executor === null ? undefined : declared.get(offer.executor) ?? known.get(offer.executor);
        const unavailable = needsCatalogue && catalogue.isPending ? t('common.loading') : t('ui.nameUnknown');
        return <li key={offer.type} className="flex min-w-0 flex-col gap-1 px-3 py-2" data-agent-arrangement>
          <span className="font-medium wrap-anywhere text-text">{arrangementName(t, offer.type)}</span>
          {offer.executor !== null && <span className="text-sm wrap-anywhere text-text-muted">{t('agents.overview.executor', { executor: executor?.title ?? unavailable })}</span>}
          {offer.toolSets.length > 0 && <CurrentDetailPage count={offer.toolSets.length}>{(from, to) => <span className="text-sm wrap-anywhere text-text-muted">{t('agents.overview.toolSets', { list: offer.toolSets.slice(from, to).map((name) => executor?.toolSets.get(name) ?? unavailable).join(t('startFlow.separator')) })}</span>}</CurrentDetailPage>}
        </li>;
      })}
    </ul>
  </>;
}

/** Opening technical details reads at most one shared catalogue, rather than one read per row. */
export function Bindings({ arrangements, program }: { arrangements: readonly ArrangementOffer[]; program: unknown }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const declared = useMemo(() => declaredExecutorNames(program, locale), [program, locale]);
  return <Section id="agent-bindings" title={<span className="inline-flex items-center gap-1">{t('agents.overview.bindings')}<HelpHint term={t('agents.overview.bindings')} guide={{ article: 'resources', heading: 'profiles-hosts-and-computers' }} what={<div className="flex flex-col gap-2"><p>{t('agents.overview.bindingsLead')}</p><ArticleGuideLink article="resources" heading="profiles-hosts-and-computers" title={t('help.hint.more')} /></div>} /></span>}>
    <CurrentDetailPage count={arrangements.length}>{(from, to) => <BindingRows offers={arrangements.slice(from, to)} declared={declared} />}</CurrentDetailPage>
  </Section>;
}
