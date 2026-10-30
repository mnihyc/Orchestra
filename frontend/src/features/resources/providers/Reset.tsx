/** Exact vendor reset review with Cancel-focused keys and one retained request identity. */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { History, RefreshCw, RotateCcw, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { useStore } from 'zustand';
import { Link } from '@tanstack/react-router';
import { ApiError, client } from '@/api/client';
import { providerKey, refreshProvider } from '@/api/resources';
import { windowName } from './details';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { formatExact, useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { AdminActNotice, adminSpec } from '@/ui/AdminAct';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Select } from '@/ui/Select';
import { consumeReset } from '@/api/provider-plan';
import type { PlanProvider, ResetBody, ResetCredit } from './plan';
import { resetUsable } from './plan';
import { spendsResets } from './details';

interface Review { provider: PlanProvider; credit: ResetCredit; body: ResetBody }
const noUse = (outcome: string) => ['nothingToReset', 'noCredit', 'alreadyRedeemed'].includes(outcome);

export function ProviderReset({ provider, onHistory }: { provider: PlanProvider; onHistory?: () => void }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const cache = useQueryClient();
  const session = useStore(client.session, (state) => state.status);
  const [open, setOpen] = useState(false);
  const [review, setReview] = useState<Review | null>(null);
  const [unknown, setUnknown] = useState(false);
  const [stale, setStale] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const manager = session === 'signed_in' && holds(provider.access, 'provider.manage');
  const listed = provider.facts.available_resets?.credits ?? [];
  const count = provider.facts.available_resets?.available_count ?? null;
  const capture = (credit: ResetCredit) => {
    if (count === null) return;
    setReview({ provider: structuredClone(provider), credit: { ...credit }, body: { request_id: newActKey(), reset_id: credit.id, available_count: count } });
    setStale(false);
    setResult(null);
  };
  const refresh = useMutation({ mutationFn: () => refreshProvider(provider.id), onSuccess: () => { setStale(true); } });
  const use = useMutation({ mutationFn: (body: ResetBody) => consumeReset(provider.id, body), onSuccess: (reply) => {
    setResult(reply.outcome);
    setUnknown(false);
  }, onError: (error) => {
    if (error instanceof ApiError && error.code === 'provider.reset_stale') {
      setStale(true);
      const current = (error.problem as unknown as Record<string, unknown>).provider;
      if (typeof current === 'object' && current !== null) cache.setQueryData(providerKey(provider.id), current);
      else void cache.invalidateQueries({ queryKey: providerKey(provider.id) });
    } else if (!(error instanceof ApiError)) setUnknown(true);
  } });

  if (!spendsResets(provider)) return null;
  const last = provider.last_reset;
  const canReview = manager && count !== null && count > 0;
  const credit = review?.credit;
  const expired = credit === undefined || !resetUsable(credit, now);
  const changed = review !== null && (provider.revision !== review.provider.revision || provider.facts.available_resets?.available_count !== review.body.available_count);
  const consumed = result !== null;
  const nothingUsed = result !== null && noUse(result);
  return <div className="flex flex-col gap-2">
    <ProviderGuideLink guide="provider-resets" />
    <Link to="/trace/$nodeId" params={{ nodeId:provider.home.id }} search={{ action:'provider.reset_used' }} className="self-start text-sm text-accent underline">{t('providerPlan.reset.history')}</Link>
    {last !== null && <p className="text-xs text-text-muted">{t('providerPlan.reset.last', { name: last.user.display_name, when: formatExact(last.at, format) })}{onHistory !== undefined && <Button size="sm" variant="ghost" icon={<History aria-hidden />} onClick={onHistory}>{t('providerPlan.reset.history')}</Button>}</p>}
    {(canReview || (manager && unknown)) && <Button size="sm" className="self-start" icon={<RotateCcw aria-hidden />} onClick={() => {
      if (!unknown) {
        use.reset();
        const first = listed.find((entry) => resetUsable(entry, now));
        if (first !== undefined) capture(first);
        else setReview(null);
      }
      setOpen(true);
    }}>{t(unknown ? 'providerPlan.reset.check' : 'providerPlan.reset.open')}</Button>}
    <Dialog open={open} onOpenChange={(next) => !use.isPending && setOpen(next)} title={t('providerPlan.reset.title')} description={t('providerPlan.reset.about')} dismissible={!use.isPending} footer={<>
      <Button data-autofocus icon={<X aria-hidden />} disabled={use.isPending} onClick={() => setOpen(false)}>{t('common.cancel')}</Button>
      {unknown && review !== null ? <Button icon={<RefreshCw aria-hidden />} pending={use.isPending} disabled={!manager} onClick={() => use.mutate(review.body)}>{t('providerPlan.reset.check')}</Button> : !consumed && <Button variant="primary" icon={<RotateCcw aria-hidden />} pending={use.isPending} disabled={!manager || expired || stale || changed || review === null} onClick={() => review !== null && use.mutate(review.body)}>{t('providerPlan.reset.confirm')}</Button>}
    </>}>
      <div className="flex flex-col gap-4" onKeyDownCapture={(event) => { if (event.key === 'Enter' && event.nativeEvent.isComposing) event.preventDefault(); }}>
        <AdminActNotice spec={adminSpec(provider, 'provider.manage')} />
        {!manager && <InlineAlert tone="warning">{t('providerPlan.reset.refused')}</InlineAlert>}
        {unknown ? <InlineAlert tone="warning">{t('providerPlan.reset.unknown')}</InlineAlert> : consumed ? <InlineAlert tone={result === 'reset' ? 'success' : 'info'}>{t(result === 'reset' ? 'providerPlan.reset.success' : nothingUsed ? 'providerPlan.reset.none' : 'providerDetails.resetReported')}</InlineAlert> : null}
        {result !== null && <p className="text-sm text-text-muted">{t('providerPlan.reset.native', { outcome: result })}</p>}
        {stale || (changed && !consumed && !unknown) ? <InlineAlert tone="warning" action={<Button size="sm" icon={<RefreshCw aria-hidden />} onClick={() => { setOpen(false); setReview(null); use.reset(); }}>{t('providerPlan.reset.review')}</Button>}>{t('providerPlan.reset.stale')}</InlineAlert> : null}
        {listed.length === 0 && !unknown ? <EmptyState compact title={t('providerPlan.reset.detailsMissing')} action={<Button size="sm" icon={<RefreshCw aria-hidden />} pending={refresh.isPending} onClick={() => refresh.mutate()}>{t('providers.overview.refresh')}</Button>} /> : <>
          {!unknown && !consumed && <Select label={t('providerPlan.reset.open')} value={review?.credit.id ?? ''} onValueChange={(id) => { const chosen = listed.find((entry) => entry.id === id); if (chosen !== undefined) capture(chosen); }} options={listed.map((entry) => ({ value: entry.id, label: entry.title ?? t('providerPlan.reset.unknownTitle'), description: entry.expires_at === null ? t('providerPlan.reset.lapseUnknown') : t('providerPlan.reset.lapse', { when: formatExact(entry.expires_at, format) }), disabled: !resetUsable(entry, now) }))} />}
          {review !== null && <>
            <p className="break-words text-base font-semibold text-text">{review.credit.title ?? t('providerPlan.reset.unknownTitle')}</p>
            {review.credit.description !== null && <p className="text-sm text-text-muted">{review.credit.description}</p>}
            <p className="text-sm text-text-muted">{review.credit.expires_at === null ? t('providerPlan.reset.lapseUnknown') : t('providerPlan.reset.lapse', { when: formatExact(review.credit.expires_at, format) })}</p>
            <p className="text-sm text-text">{t('providerPlan.reset.counts', { before: review.body.available_count, after: Math.max(0, review.body.available_count - 1) })}</p>
            <div className="flex flex-col gap-2"><h3 className="text-sm font-medium text-text">{t('providerPlan.reset.affected')}</h3><ul className="flex flex-col gap-2">{review.provider.facts.windows.map((window) => <li key={window.id} className="text-sm text-text-muted">{t('providerPlan.reset.present', { label: windowName(window), percent: formatNumber(window.used_percent, format.locale, { maximumFractionDigits: 2 }) })}</li>)}</ul>{review.provider.facts.windows.length === 0 && <EmptyState compact title={t('providerPlan.reset.windowsMissing')} />}</div>
            <p className="break-all text-xs text-text-subtle">{t('providerPlan.reset.receipt', { id: review.body.request_id })}</p>
          </>}
        </>}
        {use.error !== null && !stale && !unknown && <ErrorAlert error={use.error} />}
        {refresh.error !== null && <ErrorAlert error={refresh.error} />}
      </div>
    </Dialog>
  </div>;
}

