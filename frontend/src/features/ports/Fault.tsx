/** Safe Ports refusals remain at their act, including the deciding holders in a server problem. */
import { RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import type { Holder, NodeRef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState, MissingAccessList } from '@/ui/ErrorState';
import { PortHolders, PortScope } from './Policy';

export function PortFault({ error, onRefresh }: { error: unknown; onRefresh?: () => void }) {
  const { t } = useTranslation();
  if (error === null || error === undefined) return null;
  if (!(error instanceof ApiError)) return <ErrorAlert error={error} />;
  const scope = error.problem.scope as NodeRef | null | undefined;
  const holders = error.problem.holders as Holder[] | undefined;
  const missing = error.extra('authz.denied')?.missing ?? [];
  const hasRecovery = scope !== undefined && scope !== null || holders !== undefined || missing.length > 0 || onRefresh !== undefined;
  if (!hasRecovery) return <ErrorAlert error={error} />;
  return <ErrorState error={error} compact recovery={{ label: t('ui.error.reviewAccess'), content: <>
      {scope !== undefined && scope !== null && <PortScope scope={scope} />}
      {holders !== undefined && missing.length === 0 && <PortHolders holders={holders} />}
      <MissingAccessList missing={missing} />
      {onRefresh !== undefined && <Button size="sm" className="self-start" icon={<RefreshCw aria-hidden />} onClick={onRefresh}>{t('ports.refresh')}</Button>}
    </> }} />;
}
