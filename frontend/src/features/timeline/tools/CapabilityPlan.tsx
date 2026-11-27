/** A result viewer reads cached subscription observations under their own current provider access. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { providerQuery } from '@/api/resources';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { privateProvider } from '@/features/resources/providers/plan';
import { PathLink } from '@/shell/TrailLink';
import { ErrorState } from '@/ui/ErrorState';

export function CapabilityPlan({ providerId }: { providerId: string }) {
  const { t } = useTranslation();
  const read = useQuery({ ...providerQuery(providerId), retry: false });
  const denied = read.error instanceof ApiError && [401, 403, 404].includes(read.error.status);
  if (denied || read.data !== undefined && privateProvider(read.data)) return <span className="text-xs text-text-muted">{t('providerPlan.roomHidden')}</span>;
  if (read.isError) return <ErrorState compact error={read.error} retrying={read.isRefetching} onRetry={() => void read.refetch()} />;
  if (read.data !== undefined && read.data.kind !== 'subscription') return <span className="text-xs text-text-muted">{t('providerPlan.notReported')}</span>;
  return <div className="flex max-h-64 min-w-0 flex-col gap-2 overflow-y-auto">
    {read.data !== undefined && <PathLink className="w-fit text-sm text-accent-text hover:underline" href={`/resources/providers/${providerId}`}>{read.data.name}</PathLink>}
    <ProviderUsageCard provider={read.data} compact allWindows loading={read.isPending} />
  </div>;
}
