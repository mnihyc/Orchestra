/** The shared confirmation display consumes only the admitted preview's genuine availability. */
import { useTranslation } from 'react-i18next';
import type { AgentAccessChoice, AgentAccessWhere, ShareParts } from '@/generated/api-types';
import { AgentAccessControl, AgentAccessSummary } from '@/features/access/AgentAccessChoice';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export interface CreationAccessView {
  value: AgentAccessChoice; preset: AgentAccessChoice; reduced: boolean; ready: boolean; error: unknown; unavailable: boolean; pending: boolean; blocked?: boolean;
  available: (where: AgentAccessWhere) => ShareParts | undefined;
  onChange: (choice: AgentAccessChoice) => void; onReachChange?: (where: AgentAccessWhere) => void; onClose?: () => void; retry: () => void;
}

export function CreationAgentAccess({ access, disabled = false }: { access: CreationAccessView; disabled?: boolean }) {
  const { t } = useTranslation();
  return <div className="flex flex-col gap-2">
    <h3 className="text-base font-semibold text-text">{t('agentAccess.title')}</h3>
    {access.pending && <SkeletonRows rows={1} />}
    {access.reduced && <p className="text-sm text-text-muted" data-agent-access-reduced><span>{t('startImplicit.reducedTo')}</span> <AgentAccessSummary value={access.value} /> · <span>{t('startImplicit.byAvailableAccess')}</span></p>}
    <AgentAccessControl value={access.value} disabled={disabled} onChange={access.onChange} available={access.available} onReachChange={access.onReachChange} onClose={access.onClose} />
    {access.unavailable && <p className="text-sm text-text-muted">{t('agentAccess.presetUnavailable')}</p>}
    {access.blocked === true && <p className="text-sm text-text-muted">{t('agentAccess.unavailable')}</p>}
    {access.error !== null && <ErrorState compact error={access.error} onRetry={access.retry} />}
  </div>;
}
