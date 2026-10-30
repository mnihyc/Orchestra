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
import { EmptyState } from '@/ui/EmptyState';

import { limitsQuery, parametersQuery } from '@/api/threads';
import { EnvironmentStartRecovery } from './EnvironmentStartRecovery';
import { ThreadCallRecovery } from './CallRecovery';
import { changeModelChoice, isSettingConflict, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import { useThreadPart } from '@/realtime/stores/thread';
import { ErrorAlert } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { openChange } from '@/features/change/target';
import type { Thread } from '@/generated/api-types';
import { isAgentHistoryRemoving, waitingWords } from '@/features/timeline/waiting';
import { SandboxActions } from '@/features/resources/SandboxReadiness';
import { holds } from '@/lib/permissions';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { providerQuery } from '@/api/resources';
import { ProviderReset } from '@/features/resources/providers/Reset';
import { ApiError } from '@/api/client';
import { Link } from '@tanstack/react-router';

/** The reasons a thread is held back that another provider, profile or environment gets it out of, with what the act is called for each. */
const SWITCH_WAYS: Readonly<Record<string, 'switch' | 'switchProfile' | 'switchProvider'>> = {
  agent_offline: 'switch',
  agent_disabled: 'switchProfile',
  binding_invalid: 'switchProfile',
  provider_exhausted: 'switchProvider',
  provider_unavailable: 'switchProvider',
  model_not_allowed: 'switchProvider',
};

/**
 * The plan's other models, where the limit of one family of models stopped the thread's last turn: only those the
 * thread may use and that are not of that family, chosen by whoever manages the thread. The family is the one the
 * adapter read from the vendor's fields, never from its words; without one, every other model is offered.
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
  const parameters = useQuery({ ...parametersQuery(thread.id), enabled: waits });
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
  if (!waits || parameters.data === undefined) return null;
  const others = parameters.data.model.options.filter(
    (option) => option.allowed && option.model_name !== model && (family === '' || !option.model_name.toLowerCase().includes((family ?? '').toLowerCase())),
  );
  if (others.length === 0) return <EmptyState compact title={t('thread.blocked.familyNoOther')} />;
  const currentModel = parameters.data.model.value;
  const selected = others.some((option) => option.model_name === currentModel) ? currentModel ?? '' : '';
  return (
    <div className="flex flex-col gap-1" data-family-limit>
      <Select<string>
        size="sm"
        label={t('thread.blocked.familyChoose')}
        value={selected}
        open={open}
        onOpenChange={setOpen}
        onValueChange={(value) => change.mutate(value)}
        options={others.map((option) => ({ value: option.model_name, label: option.display_name, description: option.model_name }))}
        disabled={change.isPending || !runs.ready}
      />
      <SettingConflict refusal={runs.refusal} />
      {change.error !== null && runs.refusal === null && <ErrorAlert error={change.error} />}
    </div>
  );
}

export function BlockedBanner({ thread, onProceed }: { thread: Thread; onProceed: () => void }) {
  const { t } = useTranslation();
  const blocked = thread.state === 'blocked' && thread.blocked !== null && thread.lock === null ? thread.blocked : null;
  const secondStep = blocked?.reason === 'binding_invalid' && blocked.detail === 'second_step_required';
  const limited = blocked?.reason === 'quota_exceeded' || blocked?.reason === 'provider_exhausted';
  const limits = useQuery({ ...limitsQuery(thread.id), enabled: limited && holds(thread.access, 'thread.interact') });
  const providerId = thread.binding?.provider.id ?? '';
  const provider = useQuery({ ...providerQuery(providerId),enabled:blocked?.reason === 'provider_exhausted' && providerId !== '',retry:false });
  const permitted = !(provider.error instanceof ApiError && [403,404].includes(provider.error.status)) && provider.data !== undefined && holds(provider.data.access,'provider.view');
  if (blocked === null) return null;
  if (blocked.detail === 'call_recovery') return <ThreadCallRecovery thread={thread} banner />;
  if (isAgentHistoryRemoving(thread)) return <Banner tone="info">{t('agentHistory.removing')}</Banner>;
  if (blocked.reason === 'environment_unavailable') return <EnvironmentStartRecovery key={JSON.stringify([thread.id, thread.environment?.id])} thread={thread} />;
  const may = limits.data?.reached.some((limit) => limit.can_bypass) === true;
  const manages = holds(thread.access, 'thread.manage');
  const way = secondStep ? undefined : SWITCH_WAYS[blocked.reason];
  const mayProceed = limited && may;
  return (
    <div className="flex flex-col gap-2"><Banner
      tone={blocked.reason === 'error' ? 'danger' : 'warning'}
      className="rounded-md border"
      actionBelowOnMobile
      action={
        secondStep ? <Link to="/settings/security" className="text-sm text-accent underline">{t('secondStep.setupAction')}</Link> : mayProceed || (way !== undefined && manages) ? (
          <span className="flex flex-wrap items-center gap-2">
            {way !== undefined && manages && (
              <Button size="sm" variant={mayProceed ? 'secondary' : 'primary'} onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' })}>
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
        {!secondStep && !manages && blocked.reason !== 'agent_offline' && blocked.reason !== 'quota_exceeded' && <span className="ml-1 text-text-muted">{t('thread.blocked.manager', { who: thread.owner.display_name })}</span>}
      </span>
    </Banner>
    {blocked.sandbox!==null && blocked.sandbox!==undefined && <SandboxActions value={blocked.sandbox} />}
    {blocked.reason === 'provider_exhausted' && manages && <OtherModels thread={thread} />}
    {blocked.reason === 'provider_exhausted' && permitted && <ProviderReset provider={provider.data} />}
    {blocked.reason === 'provider_unavailable' && providerId !== '' && thread.binding?.provider.status !== 'disabled' && <Link to="/resources/providers/$id" params={{ id:providerId }} search={{ tab:'key' }} className="text-sm text-accent underline">{t('providerPlan.recovery.signin')}</Link>}
    </div>
  );
}
