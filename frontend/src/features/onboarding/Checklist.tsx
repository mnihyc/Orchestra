/** Guided setup for a new team's root: observed provider and teammate steps, with portable choices and ordinary document links.
 * The org page supplies its name; only its root's owners see missing setup. All ticks come from one current read;
 * choosing a mode or closing this card changes only the person's portable interface state.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Check, ChevronDown, CircleHelp, FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { meQuery } from '@/api/auth';
import { ApiError } from '@/api/client';
import { firstStepsAccessRefused, firstStepsQuery } from '@/api/firstSteps';
import { orgsQuery } from '@/api/nodes';
import { meStateKey, meStateQuery } from '@/api/personal';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { saveFirstSteps } from '@/features/onboarding/save';
import { ConfiguredAway } from '@/features/settings/personal/away/Status';
import { ResourceCreationActions } from '@/features/resources/ResourceCreation';
import { firstStepsChoice, internalAction, needsFirstSteps, type FirstStepsChoice } from '@/features/onboarding/state';
import type { Org, FirstSteps } from '@/generated/api-types';
import { useViewChoice } from '@/lib/viewChoices';
import { boxClasses } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { actionName, type ActionId } from '@/ui/actions';
import { ErrorState } from '@/ui/ErrorState';

const FACT_KEYS = {
  choose_org: 'chooseOrg', solo_chosen: 'soloChosen', team_chosen: 'teamChosen',
  signed_in: 'signedIn', key_saved: 'keySaved', endpoint_configured: 'endpointConfigured',
  models_enabled: 'modelsEnabled', answered_once: 'answeredOnce', outward_enabled: 'configured',
  invitation_waiting: 'invitationWaiting', member_joined: 'memberJoined', unknown: 'unknown', not_done: 'notDone',
} as const;

/** A missing projection is a load failure, with its diagnostics retained and no invented access refusal. */
export function FirstStepsFailure({error,onRetry,retrying=false}:{error:unknown;onRetry?:()=>void;retrying?:boolean}) {
  const {t}=useTranslation();
  const missing=error instanceof ApiError&&error.status===404;
  return <ErrorState compact title={t(firstStepsAccessRefused(error) ? 'firstSteps.refused' : 'firstSteps.failed')} message={missing ? t('firstSteps.unavailable') : undefined} error={error} onRetry={onRetry} retrying={retrying} />;
}

/** Navigation, invitation and notification settings keep their distinct command signs. */
function stepAction(id: FirstSteps['steps'][number]['id'], done: boolean): ActionId {
  return id === 'away' ? 'notificationSettings' : id === 'teammate' && !done ? 'invite' : 'open';
}

/** Ordinary document navigation is kept outside the dismissible setup guidance. */
export function StartHere({ document }: { document: FirstSteps['start_here'] }) {
  const { t } = useTranslation();
  if (document === null || document.deleted) return null;
  return <section aria-label={t('firstSteps.startHere')} className={`${boxClasses} flex flex-wrap items-center gap-3 p-4`}>
    <FileText className="shrink-0 text-text-muted" aria-hidden />
    <div className="min-w-0 flex-1">
      <h2 className="text-sm font-medium text-text-muted">{t('firstSteps.startHere')}</h2>
      <p className="wrap-anywhere font-medium text-text">{document.title}</p>
    </div>
    <Button action="openDocument" asChild size="sm"><a href={`/doc/${encodeURIComponent(document.id)}`} aria-label={actionName('openDocument', document.title)} title={actionName('openDocument', document.title)}>{t('firstSteps.open')}</a></Button>
  </section>;
}

/** The observed list; status text never serves as an action's wording. */
export function StepList({ data, contextPath, onReadChoices }: { data: FirstSteps; contextPath?: string; onReadChoices?: () => void }) {
  const { t } = useTranslation();
  const chosen = data.work_mode !== null;
  return <ol className="flex flex-col divide-y divide-border">
    {data.steps.map((step) => {
      if (!chosen || step.id === 'team' || (data.work_mode === 'solo' && step.id === 'teammate')) return null;
      const url = step.action === null ? null : internalAction(step.action.url);
      const done = step.state === 'done';
      const action = stepAction(step.id, done);
      const actionLabel = t(`firstSteps.actions.${step.id}${done ? 'Done' : ''}`);
      const reason = step.action?.allowed === false ? step.action.reason ?? t('firstSteps.accessNeeded') : step.action?.reason;
      return <li key={step.id} data-step={step.id} className="flex flex-col gap-2 py-2 @min-[32rem]:flex-row @min-[32rem]:items-start @min-[32rem]:gap-3">
        <div className="flex min-w-0 flex-1 items-start gap-3">
          <span className="flex h-(--control-sm) w-5 shrink-0 items-center justify-center">
            <span className={`flex size-5 items-center justify-center rounded-full ${done ? 'bg-success-subtle text-success-text' : 'bg-bg-muted text-text-muted'}`}>
              {done ? <Check aria-hidden /> : step.state === 'unknown' ? <CircleHelp aria-hidden /> : <span aria-hidden className="size-2 rounded-full border border-border-strong" />}
            </span>
          </span>
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <p className="flex min-h-(--control-sm) items-center font-medium text-text">{t(`firstSteps.steps.${step.id}`)}</p>
            {step.id === 'away' && data.org !== null ? <ConfiguredAway configured={step.fact === 'outward_enabled'} nodeId={data.org.id} /> : <p className="text-sm text-text-muted">{t(`firstSteps.facts.${FACT_KEYS[step.fact]}`)}</p>}
            {step.scripted && <span className="w-fit rounded-sm bg-warning-subtle px-2 text-sm text-warning-text">{t('firstSteps.scripted')}</span>}
            {step.source !== null && !step.source.redacted && step.source.id !== data.org?.id && <p className="text-sm wrap-anywhere text-text-subtle">{t('firstSteps.observedIn', { name: step.source.name })}</p>}
            {step.id === 'away' && done && <p className="text-sm text-text-muted">{t('firstSteps.awayMeaning')}</p>}
            {reason !== null && reason !== undefined && <p className="text-sm wrap-anywhere text-text-muted">{reason}</p>}
          </div>
        </div>
        {step.id === 'provider' && !done && step.action?.allowed === true && <ResourceCreationActions {...(onReadChoices === undefined ? {} : { onReadChoices })} />}
        {!(step.id === 'provider' && !done && step.action?.allowed === true) && url !== null && url !== contextPath && step.action !== null && (step.action.allowed ?
          <Button action={action} size="sm" asChild className="self-start"><a href={url}>{actionLabel}</a></Button> :
          <Button action={action} size="sm" blocked className="self-start" aria-label={t(`firstSteps.actions.${step.id}`)}>{t(`firstSteps.actions.${step.id}`)}</Button>)}
      </li>;
    })}
  </ol>;
}

/** First steps on the org's normal home, without another setup route or editor. */
export function Checklist({ org }: { org: Org }) {
  if (org.org_kind === 'personal' || org.archived || org.parent_id !== null || org.depth !== 0 || !org.access.owns) return null;
  return <OwnedChecklist org={org} />;
}

/** Read setup only for a root's current owners; no loading card implies that setup is missing. */
function OwnedChecklist({ org }: { org: Org }) {
  const query = useQuery(firstStepsQuery(org.id));
  const data = query.data?.org?.id === org.id ? query.data : undefined;
  if (data === undefined || !needsFirstSteps(data, org) || firstStepsAccessRefused(query.error)) return null;
  return <ChecklistCard org={org} data={data} loadError={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
}

function ChecklistCard({ org, data, loadError, retrying, onRetry }: { org: Org; data: FirstSteps; loadError: unknown; retrying: boolean; onRetry: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [progressOpen, keepProgressOpen] = useViewChoice<boolean>('onboarding', 'progressOpen', true);
  const personal = useQuery(meStateQuery);
  const choice = firstStepsChoice(personal.data);
  const same = choice.org_id === org.id;
  const mode = same ? choice.work_mode : data.work_mode;
  const me = useQuery({ ...meQuery, enabled: mode === null });
  const orgs = useQuery({ ...orgsQuery, enabled: mode === null });
  const dismissed = same && choice.dismissed;
  const mutation = useMutation({
    mutationFn: saveFirstSteps,
    onMutate: () => cache.cancelQueries({ queryKey: meStateKey, exact: true }),
    onSuccess: (saved) => { cache.setQueryData(meStateKey, saved); },
  });
  const choose = (target: Org, workMode: 'team' | 'solo') => {
    mutation.mutate({ org_id: target.id, work_mode: workMode, dismissed: false }, {
      onSuccess: () => { if (target.id !== org.id) window.location.assign(`/org/${encodeURIComponent(target.address)}`); },
    });
  };
  const saveDismissal = (value: boolean) => {
    const next: FirstStepsChoice = { org_id: org.id, work_mode: mode, dismissed: value };
    mutation.mutate(next);
  };

  const contextPath = `/org/${org.address}`;
  if (personal.isPending || !needsFirstSteps({ ...data, work_mode: mode }, org)) return null;
  const solo = orgs.data?.find((candidate) => candidate.id === me.data?.personal_org_id && candidate.access.permissions.includes('org.view'));
  return <div className="flex flex-col gap-4" data-testid="first-steps">
    {loadError === null && <StartHere document={data.start_here} />}
    {!dismissed ? <section aria-label={t('firstSteps.title')} className={`${boxClasses} @container p-4`}>
      <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
        <div className="min-w-0 flex-[1_1_8rem]">
          <h2 className="text-lg font-semibold text-text">{t('firstSteps.title')}</h2>
        </div>
        <Button action="dismiss" variant="ghost" size="sm" pending={mutation.isPending} onClick={() => saveDismissal(true)}>{t('firstSteps.dismiss')}</Button>
      </div>
      {mutation.isError && <ErrorState compact title={t('firstSteps.saveFailed')} error={mutation.error} />}
      {loadError !== null ? <FirstStepsFailure error={loadError} onRetry={onRetry} retrying={retrying} /> : personal.isError && <ErrorState compact error={personal.error} onRetry={() => void personal.refetch()} retrying={personal.isRefetching} />}
        {mode === null && <div className="flex flex-col gap-3">
          <p className="text-sm text-text-muted">{t('firstSteps.chooseFirst')}</p>
          <div className="flex flex-wrap gap-2">
            <Button action="create" size="sm" onClick={() => openNodeDialog({ type: 'new-org' })}>{t('firstSteps.createTeam')}</Button>
            <Button action="chooseOrg" size="sm" variant="primary" pending={mutation.isPending} onClick={() => choose(org, 'team')}>{t('firstSteps.useTeam')}</Button>
            {solo !== undefined && <Button action="chooseOrg" size="sm" pending={mutation.isPending} onClick={() => choose(solo, 'solo')}>{t('firstSteps.workAlone')}</Button>}
          </div>
        </div>}
        {mode !== null && <details open={progressOpen} onToggle={(event) => keepProgressOpen(event.currentTarget.open)} className="group">
          <summary className="focus-ring flex cursor-pointer items-center gap-2 py-2 text-sm font-medium text-text-muted"><ChevronDown aria-hidden />{t('firstSteps.progress')}</summary>
          <StepList data={{ ...data, work_mode: mode }} contextPath={contextPath} />
        </details>}
    </section> : <Button action="show" className="self-start" size="sm" variant="ghost" pending={mutation.isPending} onClick={() => saveDismissal(false)}>{t('firstSteps.show')}</Button>}
  </div>;
}
