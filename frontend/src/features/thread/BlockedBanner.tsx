/** Actionable preflight failure reasons; for a limit that is reached, the limit prompt.
 *
 * Why a thread cannot go on, above the composer, with the act that helps: the reason in words, and,
 * where a limit is in the way, "Proceed anyway…" for whoever may let work past it. A thread held by a
 * switch, a move or an update shows its progress instead (see the lock banner). Messages sent meanwhile
 * wait and are delivered when the thread can go on, and the banner says who can change what stands in the way.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { limitsQuery, parametersQuery } from '@/api/threads';
import { EnvironmentStartRecovery } from './EnvironmentStartRecovery';
import { ThreadCallRecovery } from './CallRecovery';
import { threadSwitchRecovery } from './switchRecovery';
import { changeModelChoice, isSettingConflict, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import { useThreadPart } from '@/realtime/stores/thread';
import { ErrorAlert } from '@/ui/ErrorState';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { openChange } from '@/features/change/target';
import type { Thread } from '@/generated/api-types';
import { isAgentHistoryRemoving, isMachineProgramVersionMismatch, waitingWords } from '@/features/timeline/waiting';
import { SandboxActions } from '@/features/resources/SandboxReadiness';
import { holds } from '@/lib/permissions';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { providerQuery } from '@/api/resources';
import { ProviderReset } from '@/features/resources/providers/Reset';
import { ApiError } from '@/api/client';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime } from '@/lib/time';
import { Link } from '@tanstack/react-router';

/**
 * The plan's other permitted models after a family limit. Model options do not report vendor-family membership,
 * so their routing names cannot establish whether the reported family includes them.
 */
function OtherModels({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const last = useThreadPart(thread.id, (view) => {
    let found: { ordinal: number; state: string; model: string; family: string | null; limited: boolean } | null = null;
    for (const turn of view.turns.values()) {
      if (found !== null && turn.ordinal < found.ordinal) continue;
      const limit = turn.error?.limit ?? null;
      found = { ordinal: turn.ordinal, state: turn.state, model: turn.model, family: limit?.model_family ?? null, limited: limit?.scope === 'model_family' };
    }
    return found === null ? null : `${found.state}\u0000${found.model}\u0000${found.limited ? (found.family ?? '') : '\u0001'}`;
  });
  const [state, model, family] = last === null ? ['', '', '\u0001'] : last.split('\u0000');
  const waits = state === 'failed' && family !== '\u0001';
  const parameters = useQuery({ ...parametersQuery(thread.id, true), enabled: waits });
  const runs = useRunsWith(thread, true);
  const [open, setOpen] = useState(false);
  const change = useMutation({
    mutationFn: (value: string) => {
      const option = parameters.data?.model.options.find((candidate) => candidate.model_name === value);
      if (option === undefined) throw new Error(t('thread.blocked.familyNoOther'));
      return changeModelChoice(thread, { model: value, effort: option.parameters.default_effort ?? null, option }, runs.states, runs.settings ?? []);
    },
    onMutate: runs.clearRefusal,
    onError: async (error) => { await runs.refuse(error); if (isSettingConflict(error)) setOpen(true); },
    onSuccess: runs.clearRefusal,
  });
  if (!waits) return null;
  const reads = runs.reads ?? [parameters];
  const readError = parameters.error ?? runs.failed;
  const failed = readError !== null && readError !== undefined;
  const checking = parameters.isPending || parameters.isFetching || runs.pending || reads.some((read) => read.isFetching === true);
  const available = checking || failed ? undefined : parameters.data;
  const others = available?.model.options.filter(
    (option) => option.allowed && option.model_name !== model,
  ) ?? [];
  const reason = checking ? t('thread.blocked.familyChecking') : failed ? t('thread.blocked.familyReadFailed')
    : available === undefined ? t('thread.blocked.familyChecking') : others.length === 0 ? t('thread.blocked.familyNoOther') : undefined;
  const currentModel = available?.model.value;
  const selected = others.some((option) => option.model_name === currentModel) ? currentModel ?? '' : '';
  return (
    <div className="flex flex-col gap-1" data-family-limit>
      <ResourcePicker<string>
        pickerKey="thread.recovery-model"
        size="sm"
        label={t('thread.blocked.familyChoose')}
        placeholder={reason ?? t('thread.blocked.familyChoose')}
        value={selected}
        open={reason === undefined && open}
        onOpenChange={setOpen}
        onValueChange={(value) => change.mutate(value)}
        options={others.map((option) => ({ value: option.model_name, label: option.display_name, description: option.model_name }))}
        disabled={reason !== undefined || change.isPending || !runs.ready}
      />
      {failed && <ErrorAlert error={readError} action={<Button size="sm" pending={checking} onClick={() => {
        for (const read of reads) if (read.isError === true) void read.refetch?.();
      }}>{t('common.tryAgain')}</Button>} />}
      <SettingConflict refusal={runs.refusal} />
      {change.error !== null && runs.refusal === null && <ErrorAlert error={change.error} />}
    </div>
  );
}

export function BlockedBanner({ thread, onProceed }: { thread: Thread; onProceed: () => void }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const blocked = thread.state === 'blocked' && thread.blocked !== null && thread.lock === null ? thread.blocked : null;
  const recordedDeadline = blocked?.offline_deadline ?? null;
  const deadline = recordedDeadline?.turn_id === thread.current_turn_id ? recordedDeadline : null;
  const secondStep = blocked?.reason === 'binding_invalid' && blocked.detail === 'second_step_required';
  const limited = blocked?.reason === 'quota_exceeded';
  const limits = useQuery({ ...limitsQuery(thread.id), enabled: limited && holds(thread.access, 'thread.interact') });
  const providerId = thread.binding?.provider.id ?? '';
  const provider = useQuery({ ...providerQuery(providerId),enabled:blocked?.reason === 'provider_exhausted' && providerId !== '',retry:false });
  const permitted = !(provider.error instanceof ApiError && [403,404].includes(provider.error.status)) && provider.data !== undefined && holds(provider.data.access,'provider.view');
  if (blocked === null) return null;
  if (blocked.detail === 'call_recovery') return <ThreadCallRecovery thread={thread} banner />;
  if (isAgentHistoryRemoving(thread)) return <Banner tone="info">{t('agentHistory.removing')}</Banner>;
  if (blocked.reason === 'environment_unavailable') return <EnvironmentStartRecovery key={JSON.stringify([thread.id, thread.environment?.id])} thread={thread} />;
  const versionMismatch = blocked.reason === 'binding_invalid' && (blocked.sandbox === null || blocked.sandbox === undefined) && isMachineProgramVersionMismatch(blocked.detail ?? '');
  const versionEnvironment = versionMismatch ? thread.environment : null;
  const may = limits.data?.reached.some((limit) => limit.cap !== null && limit.can_bypass) === true;
  const manages = holds(thread.access, 'thread.manage');
  const way = threadSwitchRecovery(thread);
  const mayProceed = limited && may;
  return (
    <div className="flex flex-col gap-2"><Banner
      tone={blocked.reason === 'error' ? 'danger' : 'warning'}
      className="rounded-md border"
      actionBelowOnMobile
      action={
        secondStep ? <Link to="/settings/security" className="text-sm text-accent underline">{t('secondStep.setupAction')}</Link> : versionEnvironment !== null || mayProceed || (way !== undefined && manages) ? (
          <span className="flex flex-wrap items-center gap-2">
            {versionEnvironment !== null && <Button size="sm" variant="primary" asChild><Link to="/resources/environments/$id" params={{ id: versionEnvironment.id }} search={{ tab: 'tools' }}>{t('resourceFacts.openEnvironment')}</Link></Button>}
            {way !== undefined && manages && (
              <Button size="sm" variant={mayProceed || versionEnvironment !== null ? 'secondary' : 'primary'} onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' })}>
                {t(`change.banner.${way}`)}
              </Button>
            )}
            {mayProceed && (
              <Button size="sm" onClick={onProceed}>
                {t('thread.blocked.proceed')}
              </Button>
            )}
          </span>
        ) : undefined
      }
    >
      <span data-blocked={blocked.reason}>
        {waitingWords(thread, t)}
        {deadline !== null && <span className="block text-sm" data-offline-deadline>
          {deadline.at !== null
            ? t('thread.blocked.offlineDeadline', { when: formatDateTime(deadline.at, format) })
            : Number.isSafeInteger(deadline.after_seconds)
              ? t('thread.blocked.offlineDeadlineDuration', { seconds: deadline.after_seconds, since: formatDateTime(deadline.since, format) })
              : t('thread.blocked.offlineDeadlineBeyondCalendar', { since: formatDateTime(deadline.since, format) })}
        </span>}
        {!secondStep && !versionMismatch && !manages && blocked.reason !== 'agent_offline' && blocked.reason !== 'quota_exceeded' && blocked.reason !== 'provider_exhausted' && <span className="ml-1 text-text-muted">{t('thread.blocked.manager', { who: thread.owner.display_name })}</span>}
      </span>
    </Banner>
    {blocked.sandbox!==null && blocked.sandbox!==undefined && <SandboxActions value={blocked.sandbox} />}
    {blocked.reason === 'provider_exhausted' && manages && <OtherModels thread={thread} />}
    {blocked.reason === 'provider_exhausted' && permitted && <ProviderReset provider={provider.data} />}
    {blocked.reason === 'provider_unavailable' && providerId !== '' && thread.binding?.provider.status !== 'disabled' && <Link to="/resources/providers/$id" params={{ id:providerId }} search={{ tab:'key' }} className="text-sm text-accent underline">{t('providerPlan.recovery.signin')}</Link>}
    </div>
  );
}
