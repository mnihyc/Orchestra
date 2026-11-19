/** The ordinary one-click access action for the place whose usage is shown. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { AccessTarget } from '@/api/access';
import { nodeQuery } from '@/api/nodes';
import { openAccess } from '@/features/access/AccessDialog';
import { Button } from '@/ui/Button';

export function UsageVisibility({ nodeId }: { nodeId:string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(nodeId));
  if (node.data === undefined) return null;
  const target:AccessTarget = { kind:node.data.kind,id:nodeId,name:node.data.name };
  return <Button action="permissions" size="sm" aria-label={t('usageVisibility.open', { place: target.name })} onClick={() => openAccess(target,{ share:false })}>{t('usageVisibility.access')}</Button>;
}
