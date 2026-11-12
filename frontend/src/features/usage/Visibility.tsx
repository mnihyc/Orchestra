/** The ordinary one-click access action for the place whose usage is shown. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { AccessTarget } from '@/api/access';
import { nodeQuery } from '@/api/nodes';
import { ApiError } from '@/api/client';
import { openAccess } from '@/features/access/AccessDialog';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';

export function UsageVisibility({ nodeId }: { nodeId:string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(nodeId));
  const privateRead = node.error instanceof ApiError && [401, 403, 404, 410].includes(node.error.status);
  const target:AccessTarget | undefined = node.data === undefined || privateRead ? undefined : { kind:node.data.kind,id:nodeId,name:node.data.name };
  return <>
    <Button action="permissions" size="sm" blocked={target === undefined}
      reason={target === undefined ? node.isPending ? t('common.loading') : t('implicitAvailability.detailsUnavailable') : undefined}
      aria-label={target === undefined ? t('usageVisibility.access') : t('usageVisibility.open', { place: target.name })}
      onClick={() => { if (target !== undefined) openAccess(target,{ share:false }); }}>{t('usageVisibility.access')}</Button>
    {node.isError && !privateRead && <ErrorState compact error={node.error} onRetry={() => void node.refetch()} />}
  </>;
}
