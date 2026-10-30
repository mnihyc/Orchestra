/** Recover the existing environment or choose another place without sending the queued input again. */
import { Hint } from '@/features/help/Hint';
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { enableEnvironment,startEnvironment } from '@/api/environments';
import { openChange } from '@/features/change/target';
import { SandboxActions } from '@/features/resources/SandboxReadiness';
import { waitingWords } from '@/features/timeline/waiting';
import type { Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

/** The parent keys this surface by thread/environment, so a late refusal cannot follow a switch. */
export function EnvironmentStartRecovery({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const environment = thread.environment;
  const mayRetry = environment !== null && environment.status !== 'lost' && thread.environment_permissions.includes('environment.manage');
  const mayChoose = holds(thread.access, 'thread.manage');
  const off=environment?.status==='disabled';
  const retry = useMutation({ mutationFn: async (id: string) => { if(off) return enableEnvironment(id); return startEnvironment(id); } });
  const requested = t(off?'resourceImpact.enabling':'startRecovery.retrying', { environment: environment?.name ?? t('source.prepare.workspace') });
  const line = environment?.status === 'lost' ? t('loss.blocked') : retry.isPending ? thread.pending_inputs > 0 ? `${requested} ${t('startRecovery.held')}` : requested : waitingWords(thread, t);
  return <div className="flex min-w-0 flex-col gap-2" data-environment-start-recovery>
    <Banner tone="warning" className="rounded-md border" actionBelowOnMobile action={mayRetry || mayChoose ? <span className="flex flex-wrap items-center gap-2">
      {mayRetry && <Button {...(off?{}:{action:'retry' as const})} size="sm" pending={retry.isPending} onClick={() => retry.mutate(environment.id)}>{t(off?'resources.act.enable':'startRecovery.retry')}</Button>}
      {mayChoose && <Button size="sm" onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'environment' })}>{t(environment?.status === 'lost' ? 'loss.elsewhere' : 'startRecovery.elsewhere')}</Button>}
    </span> : undefined}>
      <span data-blocked="environment_unavailable">{line}{environment?.status === 'lost' && <Hint popover id="loss-recover" />}{!mayChoose && !mayRetry && <span className="ml-1 text-text-muted">{t('thread.blocked.manager', { who: thread.owner.display_name })}</span>}</span>
    </Banner>
    {retry.error !== null && <ErrorAlert error={retry.error} />}
    {thread.blocked?.sandbox !== null && thread.blocked?.sandbox !== undefined && <SandboxActions value={thread.blocked.sandbox} />}
  </div>;
}
