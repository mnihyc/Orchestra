/** Current invitation defaults, without promising authority on a model or an overridden page. */
import { useQuery } from '@tanstack/react-query';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { firstStepsQuery } from '@/api/firstSteps';
import type { Role, InvitationRoleSummary } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';

/** The same line is used under the role picker and beside an invitation's current preview. */
export function RoleSummary({ summary, id }: { summary: InvitationRoleSummary; id?: string }) {
  const { t } = useTranslation();
  const acts = [
    summary.start_topics ? t('firstSteps.roles.startTopics') : null,
    summary.start_threads ? t('firstSteps.roles.startThreads') : null,
    summary.send_inputs ? t('firstSteps.roles.sendInputs') : null,
    summary.answer_requests ? t('firstSteps.roles.answerRequests') : null,
  ].filter((act): act is string => act !== null);
  return <div id={id} className="flex flex-col gap-1 text-sm text-text-muted" aria-live="polite">
    <p>{t('firstSteps.roles.defaults', { acts: acts.length === 0 ? t('firstSteps.roles.viewOnly') : acts.join(t('firstSteps.roles.join')) })}</p>
    {summary.model_access_required && <p>{t('firstSteps.roles.modelAccess')}</p>}
    <p>{t('firstSteps.roles.overrides')}</p>
  </div>;
}

/** Refresh the server's current defaults for the selected role; never select or grant a role here. */
export function InvitationRoleLine({ orgId, role, id }: { orgId: string; role: Role; id?: string }) {
  const { t } = useTranslation();
  const query = useQuery(firstStepsQuery(orgId));
  const { refetch } = query;
  useEffect(() => { void refetch(); }, [orgId, role, refetch]);
  const summary = query.data?.org?.id === orgId ? query.data.invitation_roles.find((entry) => entry.role === role) : undefined;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} retrying={query.isRefetching} />;
  if (query.isPending || query.isFetching) return <p id={id} role="status" className="text-sm text-text-muted">{t('firstSteps.roles.checking')}</p>;
  if (summary === undefined) return <ErrorState compact error={new Error(t('firstSteps.roles.missing'))} message={t('firstSteps.roles.missing')} onRetry={() => void query.refetch()} retrying={query.isRefetching} />;
  return <RoleSummary summary={summary} {...(id === undefined ? {} : { id })} />;
}
