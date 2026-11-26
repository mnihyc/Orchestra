/** Provider health for the open usage panel and the status bar that observes it. */
import { useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { Gauge } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { parametersQuery } from '@/api/threads';
import { healthQuery } from '@/api/usage';
import { errorRate, rateText, timeText } from '@/features/usage/performance';
import type { Thread } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { StatusPart } from '@/ui/StatusBar';

/** An open ring or Usage panel owns health loading and its scoped live subscription. */
export function useProviderHealth(providerId: string | null, mayView: boolean) {
  return useQuery({ ...healthQuery(providerId ?? ''), enabled: providerId !== null && mayView, retry: false });
}

/** Thresholds and scope are evaluated by the API; status only observes health a visible panel has loaded. */
export function ProviderDegradedPart({ thread }: { thread: {id:string;binding:Pick<NonNullable<Thread['binding']>,'provider'>|null} }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const now = useNow();
  const navigate = useNavigate();
  const provider = thread.binding?.provider;
  const parameters = useQuery({ ...parametersQuery(thread.id), enabled: false, retry: false });
  const named = parameters.data?.provider;
  const mayView = !parameters.isError && !parameters.isPlaceholderData && provider !== undefined && named?.id === provider.id && named.access.permissions.includes('provider.view');
  const query = useQuery({ ...healthQuery(provider?.id ?? ''), enabled: false, retry: false });
  const health = query.data;
  if (!mayView || query.isError || query.isPlaceholderData || health === undefined || (!health.errors && !health.first_output)) return null;
  const label = health.errors ? t('usagePerf.degradedErrors', { provider: provider.name, rate: rateText(errorRate(health.recent), locale, t) }) : t('usagePerf.degradedFirst', { provider: provider.name, time: timeText(health.recent.first_token_ms, locale, t) });
  return <StatusPart tone="warning" icon={<Gauge size={16} aria-hidden />} hint={t('usagePerf.degradedHint')} onOpen={() => void navigate({ to: '/usage', search: { scope: `provider:${provider.id}`, view: 'requests', from: health.since.slice(0, 10), to: new Date(now).toISOString().slice(0, 10), outcome: 'errors' } })}>{label}</StatusPart>;
}
