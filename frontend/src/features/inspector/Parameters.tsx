/** The thread's resolved choices, read here and changed in the message box, and its other declared parameters.
 * Native rows keep their guarded writes, source, lock and application time. Controls the harness does not
 * declare are absent, and a thread without other parameters needs only one line to say so.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Check, Lock, SlidersHorizontal } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { programNameIn } from '@/features/resources/programNames';
import { EmptyState } from '@/ui/EmptyState';
import { choiceOption, offeredChoice } from '@/i18n/choices';

import { parametersQuery } from '@/api/threads';
import { isApprovalMode } from '@/features/composer/ApprovalMode';
import { forceLabel, kindOf, readDef } from '@/features/composer/modelState';
import { isSettingConflict, type RunsWith, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import { ChoiceHead } from '@/features/composer/SpeedAndAccess';
import { formatContext } from '@/features/models/context';
import { modelDisplayName } from '@/features/models/names';
import type { WorkspaceAccess } from '@/features/files/door';
import type { ParameterView, SettingSource, Thread } from '@/generated/api-types';
import type { ViewProps } from '@/features/workbench/views';
import { holds } from '@/lib/permissions';
import { useAppearance } from '@/lib/appearance';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { ChoiceLocator, type ChoiceLocatorOption } from '@/ui/ChoiceLocator';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

import { ThreadRules } from '@/features/approvals/ThreadRules';

import { ThreadPanel } from './ThreadPanel';
import { ProgramBuild } from '@/features/resources/agents/ProgramBuild';
import { applyWord } from './words';

/** How a parameter is declared, as far as this view reads it: the rest of the declaration is the agent's own. */
interface Declared {
  title: string;
  description: string;
  group: string;
  type: string;
  options: { value: string; label: string }[] | null;
}

function declared(parameter: ParameterView): Declared {
  const def = (typeof parameter.def === 'object' && parameter.def !== null ? parameter.def : {}) as Record<string, unknown>;
  const options = Array.isArray(def.options) ? (def.options as { value?: unknown; label?: unknown }[]).filter((one) => typeof one.value === 'string').map((one) => ({ value: String(one.value), label: typeof one.label === 'string' ? one.label : String(one.value) })) : null;
  return {
    title: typeof def.title === 'string' ? def.title : parameter.key,
    description: typeof def.description === 'string' ? def.description : '',
    group: GROUPS.find((group) => group === def.group) ?? 'advanced',
    type: typeof def.type === 'string' ? def.type : 'string',
    options,
  };
}

const GROUPS = ['model', 'behaviour', 'safety', 'advanced'] as const;

/** Where a value comes from, in words: set here, from a place above, or the default. */
function Provenance({ source }: { source: SettingSource }) {
  const { t } = useTranslation();
  const words =
    source.scope === 'thread'
      ? t('inspector.parameters.from.here')
      : source.scope === 'default'
        ? t('inspector.parameters.from.default')
        : source.scope === 'environment' || source.scope === 'environment_own'
          ? t('inspector.parameters.from.environment')
          : t('inspector.parameters.from.place', { kind: t(`inspector.parameters.scope.${source.scope === 'system' ? 'system' : source.scope === 'org' ? 'org' : source.scope === 'project' ? 'project' : source.scope === 'topic' ? 'topic' : 'default'}`), name: source.node?.name ?? '' });
  return <span className="rounded-full bg-bg-muted px-2 py-1 text-xs text-text-muted">{words}</span>;
}

/** One native parameter, with its compact control and the facts about the value in force. */
export function ParameterRow({ parameter, access, writable, runs, manageReason = true }: { parameter: ParameterView; access: { thread: Pick<Thread, 'id' | 'owner'> }; writable: boolean; runs: Pick<RunsWith, 'ready' | 'write' | 'reset' | 'refuse'>; manageReason?: boolean }) {
  const { t } = useTranslation();
  const [saved, setSaved] = useState(false);
  const [open, setOpen] = useState(false);
  const refused = async (error: unknown) => {
    await runs.refuse(error, parameter.key);
    if (isSettingConflict(error)) setOpen(true);
  };
  const info = declared(parameter);
  const set = useMutation({
    mutationFn: (value: unknown) => runs.write(parameter.key, value),
    onError: refused,
    onSuccess: () => {
      setSaved(true);
      setTimeout(() => setSaved(false), 2_000);
    },
  });
  const reset = useMutation({ mutationFn: () => runs.reset(parameter.key), onError: refused });
  const locked = parameter.locked_by !== null;
  const can = writable && runs.ready && parameter.writable && !locked;
  const why = !writable && manageReason ? t('inspector.parameters.needsManage', { name: access.thread.owner.display_name }) : null;
  const current = parameter.value ?? (typeof parameter.def === 'object' && parameter.def !== null ? (parameter.def as Record<string, unknown>).default : null);
  const shownValue = typeof current === 'string' || typeof current === 'number' ? String(current) : '';
  const isSwitch = info.type === 'bool' || info.type === 'boolean';
  return (
    <li className="flex flex-col gap-1 border-b border-border px-3 py-2 last:border-b-0" data-testid="parameter-row">
      <div className="flex flex-wrap items-center gap-2">
        {isSwitch ? <Switch className="min-w-0 flex-1" label={info.title} checked={current === true} disabled={!can || set.isPending} onCheckedChange={(on) => set.mutate(on)} />
          : <span className="min-w-0 flex-1 text-sm font-medium text-text">{info.title}</span>}
        <Provenance source={parameter.source} />
        {locked && (
          <span className="inline-flex items-center gap-1 text-xs text-text-muted" title={t('inspector.parameters.locked')}>
            <Lock aria-hidden />
            {t('inspector.parameters.lockedBy', { name: parameter.locked_by?.node?.name ?? '' })}
          </span>
        )}
        {parameter.source.scope === 'thread' && can && (
          <IconButton action="inheritSetting" size="sm" label={t('inspector.parameters.reset', { name: info.title })} pending={reset.isPending} onClick={() => reset.mutate()} />
        )}
        {saved && (
          <span role="status" className="inline-flex items-center gap-1 text-xs text-success-text">
            <Check aria-hidden />
            {t('inspector.parameters.saved')}
          </span>
        )}
      </div>
      {!isSwitch && (info.options !== null ? (
        <ChoiceLocator<string>
          size="sm"
          label={info.title}
          hideLabel
          value={typeof current === 'string' ? current : ''}
          valueLabel={info.options.some((option) => option.value === current) ? undefined : shownValue}
          open={open}
          onOpenChange={setOpen}
          onValueChange={(value) => set.mutate(value)}
          options={info.options.map((option): ChoiceLocatorOption<string> => ({ value: option.value, ...choiceOption(t, 'dynamic.value', { name: option.label, context: info.title }), description: info.description }))}
          disabled={!can || set.isPending}
        />
      ) : (
        <Input
          key={shownValue}
          label={info.title}
          hideLabel
          defaultValue={shownValue}
          readOnly={!can}
          onBlur={(event) => {
            const text = event.target.value;
            if (can && text !== shownValue) set.mutate(info.type === 'int' || info.type === 'number' ? Number(text) : text);
          }}
        />
      ))}
      <p className="text-xs text-text-subtle">
        {applyWord(t, parameter.apply)}
        {why !== null && ` · ${why}`}
      </p>
      {set.error !== null && <ErrorAlert error={set.error} />}
      {reset.error !== null && <ErrorAlert error={reset.error} />}
    </li>
  );
}

/** The inspector reads the composer's choices and edits only the other native parameters. */
export function ParametersPanel({ access }: { access: Pick<WorkspaceAccess, 'thread'> }) {
  const { t } = useTranslation();
  const query = useQuery(parametersQuery(access.thread.id));
  const runs = useRunsWith(access.thread, true);
  const locale = useAppearance((state) => state.locale);
  const writable = holds(access.thread.access, 'thread.manage');
  if (query.error !== null && query.data === undefined) return <ErrorState compact className="h-full" error={query.error} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return <SkeletonRows rows={6} />;
  const data = query.data;
  const subagentsOff = access.thread.binding?.program?.build?.check?.off.includes('subagents') === true;
  const supportsAtOnce = !subagentsOff && (data.subagent_limits?.supported.includes('at_once') ?? false);
  const supportsDepth = !subagentsOff && (data.subagent_limits?.supported.includes('depth') ?? false);
  const current = data.model.options.find((option) => option.model_name === data.model.value);
  const modelName = current?.display_name.trim();
  const modelWords = data.model.value === null ? t('inspector.parameters.modelUnset')
    : modelName === undefined || modelName === '' ? t('inspector.parameters.notReported') : modelDisplayName(modelName, t);
  const approvalWords = isApprovalMode(data.approval_mode) ? t(`composer.mode.labels.${data.approval_mode}`) : t('inspector.parameters.notReported');
  const effortWords = data.effort.value === null ? t('inspector.parameters.notReported')
    : (offeredChoice(t, 'effort', data.effort.value)?.label ?? data.effort.value);
  const efforts = current?.parameters.efforts ?? [];
  const states = runs.states.filter((state) => state.kind === 'context' || state.open.length > 0);
  const generic = data.parameters.filter((parameter) => {
    const def = readDef(parameter.def);
    return def === null || (kindOf(def) === null && def.options !== 'efforts');
  });
  const grouped = GROUPS.map((group) => ({ group, rows: generic.filter((parameter) => declared(parameter).group === group && parameter.key !== 'reasoning_effort') })).filter((entry) => entry.rows.length > 0);
  return (
    <div className="h-full overflow-auto" data-testid="parameters-panel">
      <section aria-label={t('inspector.parameters.currentChoices')} className="flex flex-col gap-2 border-b border-border px-3 py-2">
        <dl className="flex flex-col gap-2 text-sm">
          <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <dt className="font-medium text-text">{t('inspector.parameters.model')}</dt>
            <dd className="flex min-w-0 flex-wrap items-baseline gap-2 wrap-anywhere text-text-muted"><span>{modelWords}</span><Provenance source={data.model.source} /></dd>
          </div>
          <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <dt className="font-medium text-text">{t('composer.mode.title')}</dt>
            <dd className="text-text-muted">{approvalWords}</dd>
          </div>
          {data.effort.supported && efforts.length > 0 && <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
            <dt className="font-medium text-text">{t('inspector.parameters.effort')}</dt>
            <dd className="flex min-w-0 flex-wrap items-baseline gap-2 wrap-anywhere text-text-muted"><span>{effortWords}</span><Provenance source={data.effort.source} /></dd>
          </div>}
        </dl>
        {states.map((state) => <div key={state.key} className="min-w-0 wrap-anywhere"><ChoiceHead
          title={t(state.kind === 'context' ? 'composer.model.context.title' : state.kind === 'speed' ? 'composer.model.speed.title' : 'composer.model.access.title')}
          value={state.kind === 'context' ? formatContext(state.force, locale) : forceLabel(state) ?? t(state.kind === 'speed' ? 'composer.model.speed.standard' : 'composer.model.access.none')}
          state={state} chooser={runs.chooser(state.key)} /></div>)}
        <p className="text-xs text-text-muted">{t('inspector.parameters.changeInComposer')}</p>
        {data.approval_reviewer !== null && <p className="text-xs wrap-anywhere text-text-muted">
          {t('inspector.parameters.reviewer', { model: data.approval_reviewer.model })} · {t(data.approval_reviewer.usage_apart ? 'inspector.parameters.reviewUseApart' : 'inspector.parameters.reviewUseInside')}
        </p>}
      </section>
      {!writable && grouped.length > 0 && <p className="px-3 py-2 text-xs text-text-muted">{t('inspector.parameters.needsManage', { name: access.thread.owner.display_name })}</p>}
      <SettingConflict refusal={runs.refusal} />
      {grouped.length === 0 ? (
        <EmptyState compact className="px-3" title={t('inspector.parameters.noParameters')} />
      ) : (
        grouped.map(({ group, rows }) => (
          <section key={group} aria-label={t(`inspector.parameters.group.${group}`)}>
            <h3 className="bg-bg-subtle px-3 py-1 text-xs font-medium text-text-muted">{t(`inspector.parameters.group.${group}`)}</h3>
            <ul>
              {rows.map((parameter) => (
                <ParameterRow key={parameter.key} parameter={parameter} access={access} writable={writable} runs={runs} manageReason={false} />
              ))}
            </ul>
          </section>
        ))
      )}
      <ThreadRules threadId={access.thread.id} writable={writable} />
      {access.thread.binding?.program !== undefined && access.thread.binding.program !== null && <div className="px-3 py-2"><ProgramBuild program={access.thread.binding.program} name={programNameIn(access.thread.binding.agent, locale) ?? undefined} /></div>}
      {data.subagent_limits !== null && data.subagent_limits !== undefined && (supportsAtOnce || supportsDepth) && <div className="px-3 py-2 text-sm">
        {supportsAtOnce && <p>{t('inspector.parameters.subagentsAtOnce', { count: data.subagent_limits.limits.at_once })}</p>}
        {supportsDepth && <p>{t('inspector.parameters.subagentDepth', { count: data.subagent_limits.limits.depth })}</p>}
        <p className="text-text-muted">{t('inspector.parameters.subagentsApply')}</p>
      </div>}
    </div>
  );
}

/** The Parameters view of a thread. */
export function ParametersView(_props: ViewProps) {
  return (
    <ThreadPanel icon={<SlidersHorizontal aria-hidden />}>
      {(thread) => <ParametersPanel access={{ thread }} />}
    </ThreadPanel>
  );
}
