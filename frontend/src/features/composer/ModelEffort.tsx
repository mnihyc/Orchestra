/** What a thread runs with, as separate compact controls: the model, its effort, its speed and its access program; applies from the next turn.
 *
 * Each control shows its current value alone and opens a list of that one thing. The model's list holds
 * models only, one row each (its name, and in muted words the provider and the context it takes), with
 * search once more than eight are listed and its own scroll, and groups where the provider declares them;
 * the effort's list holds the efforts of the chosen model, its default marked. A model that offers only one
 * speed, or none, shows no speed; exactly two speeds are one chip that flips and says the one that is on, and
 * more are a list. The access program stands where the model's managers switched one on. The size of the
 * context is chosen from the context ring (`ContextRing`). A control shows the thread's own state and never
 * what the provider allows, which is on the provider's Models tab. Changing a model drops a choice the new one
 * does not have, with a line saying so. Changing any of it is setting the session up: somebody who only
 * drives the thread sees it read-only, with who can change it. Models that are not allowed are listed last,
 * with the reason. The choice of a model of Ask, and of a model of the dialogs that start a thread, are the
 * same controls on their own entries.
 */
import { type TFunction } from 'i18next';
import { ChevronRight, Zap } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { threadBindingOptionsQuery } from '@/api/start';
import { openChange } from '@/features/change/target';
import { effortAfterSwitch, effortGroups, groupedRows, optionName } from '@/features/composer/modelRows';
import { ProviderModelChip, type ProviderModels, providerModelsOf } from '@/features/composer/ProviderModelChip';
import { ChoiceChip, PickList, type PickGroup, type PickRow } from '@/features/composer/PickList';
import { type ControlName, type Dropped, forceLabel, type SpeedOrAccess, type State } from '@/features/composer/modelState';
import { changeModelChoice, type RunsWith, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import { NONE, lockedWords } from '@/features/composer/SpeedAndAccess';
import { programRows, speedRows, WithheldChip, withheldOf } from '@/features/composer/Withheld';
import { SettingHelpGroup } from '@/features/help/SettingHelpGroup';
import { formatContext } from '@/features/models/context';
import { accessProgramDisplayName, effortDisplayName, modelDisplayName } from '@/features/models/names';
import type { ModelOption, Thread, ThreadParameters } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { cx } from '@/ui/cx';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { toast } from '@/ui/Toast';
import { sameSelectionModel, selectionEffort, selectionWithEffort, type ModelSelection } from './modelSelection';

export interface ModelEffortProps {
  thread: Thread;
  /** The composer's controls, or the chip of a header. Both read the same way. */
  variant?: 'composer' | 'chip';
  /** Which control is open, where the box opens one by a command of its own; none when none is. */
  open?: ControlName | null;
  onOpenChange?: (open: ControlName | null) => void;
  /** Fits the composer's phone grid without hiding its delivery controls. */
  compact?: boolean;
  /** A watch-only row never changes a parameter, even when another permission could edit it. */
  readOnly?: boolean;
}

/** An unset model stays empty; a missing display name never turns a routing key into a title. */
export const modelName = (thread: Pick<Thread, 'model' | 'model_display'>): string => thread.model === null ? '' : modelDisplayName(thread.model_display);

/** What the control says a thread runs with beyond its model and effort: the speed and the access program where they are not the ordinary ones, and the context where a choice of it is open. */
export function runsWithParts(states: readonly State[], locale: string): string[] {
  const parts: string[] = [];
  for (const state of states) {
    if (state.kind === 'context') parts.push(formatContext(state.force, locale));
    else {
      const label = forceLabel(state);
      if (label !== null) parts.push(label);
    }
  }
  return parts;
}

/** What a change of model left behind, in a sentence for each choice. */
export function leftBehind(dropped: Dropped, next: ModelOption, locale: string, t: TFunction): string {
  const model = optionName(next);
  if (dropped.kind === 'speed') {
    const becomes = dropped.becomes === null ? t('composer.model.speed.standard') : ((next.parameters.service_tiers ?? []).find((tier) => tier.id === dropped.becomes)?.label ?? String(dropped.becomes));
    return t('composer.model.dropped.speed', { model, was: dropped.was, becomes });
  }
  if (dropped.kind === 'access') {
    const was = accessProgramDisplayName(String(dropped.was), t);
    return dropped.becomes === null ? t('composer.model.dropped.accessNone', { model, was }) : t('composer.model.dropped.access', { model, was, becomes: accessProgramDisplayName(String(dropped.becomes), t) });
  }
  const was = typeof dropped.was === 'number' ? formatContext(dropped.was, locale) : dropped.was;
  return typeof dropped.becomes === 'number'
    ? t('composer.model.dropped.context', { model, was, becomes: formatContext(dropped.becomes, locale) })
    : t('composer.model.dropped.contextNone', { model, was });
}

/** The speeds of a state as a list: the ordinary speed, where it may be chosen, and each one that is switched on. */
export function speedChoices(state: SpeedOrAccess, standard: string): { id: string | null; label: string }[] {
  return [...(state.allowNone === false ? [] : [{ id: null, label: standard }]), ...state.open];
}

/** The access programs of a state as a list: none, where it may be chosen, and each one that is switched on. */
export function programChoices(state: SpeedOrAccess, none: string): { id: string | null; label: string }[] {
  return [...(state.allowNone === false ? [] : [{ id: null, label: none }]), ...state.open];
}

const valueOf = (id: string | null): string => id ?? NONE;

/** Speed is hidden without alternatives; an active access program remains readable even when fixed. */
function ChoiceOfState({ state, what, choices, writable, reason, runs, open, onOpenChange, control, flip }: {
  state: SpeedOrAccess; what: string; choices: { id: string | null; label: string }[]; writable: boolean; reason: string | undefined; runs: RunsWith;
  open: boolean; onOpenChange: (open: boolean) => void; control: string; flip: boolean;
}) {
  const { t } = useTranslation();
  if (choices.length === 0 || choices.length < 2 && state.kind === 'speed') return null;
  const inForce = choices.find((choice) => choice.id === state.force) ?? choices[0];
  const value = inForce?.label ?? '';
  const choose = (id: string | null) => { if (writable && id !== state.force) runs.choose(state, id); };
  const rows: PickRow[] = choices.map((choice) => ({ id: valueOf(choice.id), name: choice.label, checked: choice.id === state.force, disabled: !writable,
    detail: choice.id === state.start ? t('composer.chip.startsWith') : undefined }));
  const onChoose = (row: PickRow) => { choose(row.id === NONE ? null : row.id); onOpenChange(false); };
  if (flip && choices.length === 2) {
    const other = choices.find((choice) => choice.id !== state.force) ?? choices[1];
    return <span className="inline-flex min-w-0 max-w-full" data-composer-chip={control}>
      <Popover label={what} open={open} onOpenChange={onOpenChange} side="top" scroll="children" trigger={<Button size="sm" variant="ghost" bounded blocked={!writable} reason={reason} aria-pressed={state.force !== null} aria-label={t('composer.chip.value', { what, value })}
        icon={control === 'speed' ? <Zap aria-hidden /> : undefined} className="min-w-0 max-w-(--chip-width) gap-1 px-2 text-sm"
        onClick={(event) => { event.preventDefault(); if (other !== undefined) choose(other.id); onOpenChange(false); }}>
        <span className="min-w-0 truncate">{value}</span>
      </Button>}>
        <PickList label={what} groups={[{ id: control, label: null, rows }]} onChoose={onChoose} />
      </Popover>
    </span>;
  }
  return <ChoiceChip what={what} value={value} reason={reason} disabled={choices.length < 2} open={open} onOpenChange={onOpenChange} control={control}
    groups={[{ id: control, label: null, rows }]}
    onChoose={onChoose} />;
}

/** The model, effort, speed and access program of a thread, as controls of its own. */
function ThreadRunsChips({ thread, open, onOpenChange, readOnly }: Pick<ModelEffortProps, 'thread' | 'open' | 'onOpenChange'> & { readOnly: boolean }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const runs = useRunsWith(thread, true);
  const [local, setLocal] = useState<ControlName | null>(null);
  const current = open === undefined ? local : open;
  const show = (name: ControlName) => (next: boolean) => { const value = next ? name : null; setLocal(value); onOpenChange?.(value); };
  const mayChange = holds(thread.access, 'thread.manage');
  const change = useMutation({
    mutationFn: (choice: { model: string; effort: string | null; option: ModelOption; effortOnly?: boolean }) => changeModelChoice(thread, choice, runs.states, runs.settings ?? []),
    onMutate: runs.clearRefusal,
    // A refusal stays in the list, with who changed the model and what it is now; the choice is made again from there.
    onError: async (error) => { await runs.refuse(error); },
    onSuccess: ({ dropped, option }) => {
      const lines = dropped.map((one) => leftBehind(one, option, locale, t));
      if (lines.length > 0) toast({ tone: 'info', title: t('composer.model.dropped.title'), description: lines.join(' ') });
      setLocal(null); onOpenChange?.(null);
    },
  });
  const parameters: ThreadParameters | undefined = runs.parameters;
  const bound = thread.binding !== null;
  // The models of every provider the person may choose, read once the list is opened.
  const [opened, setOpened] = useState(false);
  const wide = useQuery({ ...threadBindingOptionsQuery(thread.id), enabled: opened && mayChange && bound && !readOnly });
  const here = thread.binding?.provider;
  const providers: ProviderModels[] = wide.data !== undefined ? providerModelsOf(wide.data).map((entry) => entry.provider.id === here?.id && parameters !== undefined ? { ...entry, models: parameters.model.options } : entry)
    : here === undefined ? [] : [{ provider: { id: here.id, name: here.name, address: here.address, status: 'ready', usable: true, reason: null, owner: null }, models: parameters?.model.options ?? [] }];
  const modelValue = parameters?.model.value ?? thread.model;
  const model = parameters?.model.options.find((option) => option.model_name === modelValue);
  const name = parameters === undefined || model === undefined ? modelName(thread) : optionName(model);
  const canWrite = mayChange && !readOnly && runs.ready && !change.isPending && !runs.choosing;
  const writable = canWrite && parameters?.model.writable === true;
  const effortWritable = canWrite && parameters?.effort.writable === true;
  const efforts = parameters?.effort.supported ? model?.parameters.efforts ?? [] : [];
  const effort = parameters?.effort.value ?? thread.effort ?? model?.parameters.default_effort ?? null;
  // Where what the model lists is not read (a viewer's, or not yet), the effort the thread runs with stands as a plain chip.
  const unread = effort !== null && (parameters === undefined || (model === undefined && parameters.effort.supported));
  const readReason = readOnly ? t('composer.row.readOnly') : !mayChange ? t('composer.model.readOnly', { who: thread.owner.display_name }) : undefined;
  const blocked = (
    <>
      {runs.refusal !== null && runs.refusal.member === undefined && <SettingConflict refusal={runs.refusal} />}
      {change.error !== null && runs.refusal === null && <ErrorAlert error={change.error} />}
    </>
  );
  const states = runs.states;
  const speed = states.find((state): state is SpeedOrAccess => state.kind === 'speed');
  const access = states.find((state): state is SpeedOrAccess => state.kind === 'access');
  // What the model's managers opened and this thread cannot use stands where the working control would, dimmed, with why.
  const offSpeed = speed === undefined ? withheldOf(parameters, 'speed') : undefined;
  const offAccess = access === undefined ? withheldOf(parameters, 'access') : undefined;
  const helpSettings = ['agent.model', ...(efforts.length > 0 ? ['agent.effort'] : []), ...(states.length > 0 ? ['agent.params'] : [])];
  const stateWritable = (state: SpeedOrAccess) => mayChange && !readOnly && state.writable && state.providerFixed !== true && runs.ready && !runs.choosing && !change.isPending;
  return (
    <>
      <ProviderModelChip what={t('composer.model.chip')} name={name} disabled={!bound || readOnly} reason={readReason}
        open={current === 'model'} onOpenChange={(next) => { if (next) setOpened(true); show('model')(next); }}
        providers={providers} current={{ provider: here?.id ?? '', model: modelValue }} locked={!writable} pending={runs.pending || wide.isPending && opened && mayChange} failed={wide.isError || runs.failed !== null && runs.failed !== undefined}
        searchLabel={t('composer.model.find')}
        above={!mayChange ? <p className="px-2 text-sm text-text-muted">{t('composer.model.readOnly', { who: thread.owner.display_name })}</p> : undefined}
        below={<>{blocked}<span className="flex justify-end px-1"><SettingHelpGroup settingKeys={helpSettings} target={{ kind: 'node', id: thread.id }} term={t(efforts.length > 0 ? 'helpGroups.modelAndEffort' : 'helpGroups.model')}
          article="resources" heading="providers-and-models" /></span></>}
        onModel={(provider, option, close) => {
          if (provider.id !== here?.id) {
            // Another provider is a change of what the thread runs on, which the wizard plans: it opens with this choice made.
            openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider', provider: provider.id, model: option.model_name });
            close();
            return;
          }
          change.mutate({ model: option.model_name, effort: effortAfterSwitch(effort, option), option });
        }} />
      {((efforts.length > 0 && model !== undefined) || unread) && <ChoiceChip what={t('composer.effort.title')} value={effort === null ? '' : effortDisplayName(effort, t)} control="effort" disabled={readOnly || unread}
        reason={readReason} open={current === 'effort'} onOpenChange={show('effort')}
        groups={model === undefined ? [] : effortGroups(model, effort, t, !effortWritable)} below={blocked}
        above={!mayChange ? <p className="px-2 text-sm text-text-muted">{t('composer.model.readOnly', { who: thread.owner.display_name })}</p> : undefined}
        onChoose={(row) => {
          if (!effortWritable || model === undefined) return;
          change.mutate({ model: model.model_name, effort: row.id, option: model, effortOnly: true });
        }} />}
      {speed !== undefined && <ChoiceOfState state={speed} what={t('composer.model.speed.title')} choices={speedChoices(speed, t('composer.model.speed.standard'))} flip
        writable={stateWritable(speed)} reason={stateWritable(speed) ? undefined : lockedWords(speed, readOnly ? t('composer.row.readOnly') : null, mayChange ? null : t('composer.model.readOnly', { who: thread.owner.display_name }), t)}
        runs={runs} open={current === 'speed'} onOpenChange={show('speed')} control="speed" />}
      {access !== undefined && <ChoiceOfState state={access} what={t('composer.model.access.title')} choices={programChoices(access, t('composer.model.access.none'))} flip={false}
        writable={stateWritable(access)} reason={stateWritable(access) ? undefined : lockedWords(access, readOnly ? t('composer.row.readOnly') : null, mayChange ? null : t('composer.model.readOnly', { who: thread.owner.display_name }), t)}
        runs={runs} open={current === 'access'} onOpenChange={show('access')} control="access" />}
      {offSpeed !== undefined && model !== undefined && <WithheldChip thread={thread} entry={offSpeed} what={t('composer.model.speed.title')} control="speed" rows={speedRows(model.parameters, t('composer.model.speed.standard'))} />}
      {offAccess !== undefined && model !== undefined && <WithheldChip thread={thread} entry={offAccess} what={t('composer.model.access.title')} control="access" rows={programRows(model.parameters, t('composer.model.access.none'), t)} />}
      {runs.chooseError !== null && runs.refusal === null && <ErrorAlert error={runs.chooseError} />}
      {runs.refusal !== null && runs.refusal.member !== undefined && <SettingConflict refusal={runs.refusal} />}
      {runs.failed !== null && runs.failed !== undefined && <ErrorAlert error={runs.failed} action={<Button size="sm" onClick={() => {
        for (const read of runs.reads ?? []) if (read.isError === true) void read.refetch?.();
      }}>{t('common.tryAgain')}</Button>} />}
      {wide.isError && <ErrorAlert error={wide.error} action={<Button size="sm" onClick={() => { void wide.refetch(); }}>{t('common.tryAgain')}</Button>} />}
    </>
  );
}

/** IDs are the producer's row identities; equal models may have distinct ordered presets. */
export interface ModelEffortEntry {
  id: string;
  value: ModelSelection;
  option: ModelOption;
  providerName: string;
  reason?: ReactNode;
}
export interface ControlledModelEffortProps {
  choice: {
    value: ModelSelection | null;
    entries: readonly ModelEffortEntry[];
    onChange: (value: ModelSelection | null, entry?: ModelEffortEntry) => void;
    label: string;
    selectedId?: string | null;
    defaultChoice?: { label: string; value: ModelSelection | null };
    disabled?: boolean;
    showEffort?: boolean;
    state?: ReactNode;
    renderParameters?: (entry: ModelEffortEntry, value: ModelSelection, onChange: (params: unknown) => void, disabled: boolean) => ReactNode;
  };
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

const DEFAULT_ROW = '$default';

/** The model and effort of a choice that is not a thread's: Ask's, and the dialogs that start a thread. The same two controls on the entries it is given. */
function ControlledModelEffort({ choice, open: controlledOpen, onOpenChange }: ControlledModelEffortProps) {
  const { t } = useTranslation();
  const [localOpen, setLocalOpen] = useState(false);
  const [effortOpen, setEffortOpen] = useState(false);
  const open = controlledOpen ?? localOpen;
  const show = (next: boolean) => { setLocalOpen(next); onOpenChange?.(next); };
  const effective = choice.value ?? choice.defaultChoice?.value;
  const selected = choice.entries.find((entry) => entry.id === choice.selectedId)
    ?? choice.entries.find((entry) => sameSelectionModel(effective, entry.value) && JSON.stringify(entry.value.params) === JSON.stringify(effective?.params))
    ?? choice.entries.find((entry) => sameSelectionModel(effective, entry.value));
  const rawEffort = selectionEffort(effective?.params);
  const presetEffort = selectionEffort(selected?.value.params);
  const effort = rawEffort !== undefined ? rawEffort : presetEffort !== undefined ? presetEffort : selected?.option.parameters.default_effort ?? null;
  const name = selected === undefined ? effective === null || effective === undefined ? '' : modelDisplayName(undefined, t) : optionName(selected.option);
  const defaultValue = choice.defaultChoice?.value;
  const defaultEntry = choice.entries.find((entry) => sameSelectionModel(defaultValue, entry.value) && JSON.stringify(entry.value.params) === JSON.stringify(defaultValue?.params))
    ?? choice.entries.find((entry) => sameSelectionModel(defaultValue, entry.value));
  const defaultName = defaultEntry === undefined ? defaultValue === null || defaultValue === undefined ? '' : modelDisplayName(undefined, t) : optionName(defaultEntry.option);
  const value = choice.value === null && choice.defaultChoice !== undefined ? [choice.defaultChoice.label, name || defaultName].filter(Boolean).join(' · ') : name;
  const choose = (entry: ModelEffortEntry, selectedEffort?: string) => {
    if (choice.disabled || !entry.option.allowed) return;
    const base = selected?.id === entry.id && choice.value !== null ? choice.value : entry.value;
    choice.onChange(selectedEffort === undefined ? { ...entry.value } : { ...base, params: selectionWithEffort(base.params, selectedEffort) }, entry);
  };
  const usable = choice.entries.filter((entry) => entry.option.allowed).map((entry) => ({ group: entry.option.groups?.[0], row: { id: entry.id, name: optionName(entry.option), keywords: entry.option.model_name,
    checked: choice.value !== null && selected?.id === entry.id, disabled: choice.disabled === true, detail: entry.providerName } satisfies PickRow }));
  const groups: PickGroup[] = [
    ...(choice.defaultChoice === undefined ? [] : [{ id: 'default', label: null, rows: [{ id: DEFAULT_ROW, name: choice.defaultChoice.label, checked: choice.value === null, disabled: choice.disabled === true, detail: defaultName }] }]),
    ...groupedRows(usable),
    { id: 'closed', label: t('composer.model.notAllowed'), rows: choice.entries.filter((entry) => !entry.option.allowed).map((entry): PickRow => ({ id: entry.id, name: optionName(entry.option), disabled: true,
      detail: entry.reason ?? t(`composer.model.reason.${entry.option.reason ?? 'disabled'}`, { provider: entry.providerName }) })) },
  ];
  const parameters = selected !== undefined && selected.option.allowed && effective !== undefined && effective !== null ? choice.renderParameters?.(selected, effective,
    (params) => { if (!choice.disabled) choice.onChange({ ...effective, params }, selected); }, choice.disabled === true) : null;
  // Options most people never change stay closed until asked for, so the models keep the room.
  const options = parameters === null || parameters === undefined || parameters === false ? null : <details className="group min-w-0 shrink-0">
    <summary className="focus-inset flex cursor-default list-none items-center gap-2 rounded-md py-1 text-sm text-text-muted select-none hover:bg-bg-subtle [&::-webkit-details-marker]:hidden">
      <ChevronRight className="shrink-0 text-text-subtle transition-transform group-open:rotate-90" size={14} aria-hidden />{t('providerOptions.title')}</summary>
    <div className="min-w-0 pt-2">{parameters}</div>
  </details>;
  const efforts = choice.showEffort === false || !selected?.option.allowed ? [] : selected.option.parameters.efforts ?? [];
  return <span className="inline-flex min-w-0 max-w-full items-center gap-1">
    <ChoiceChip what={choice.label} ariaLabel={choice.label} value={value} control="model" disabled={choice.disabled === true} open={open} onOpenChange={show}
      groups={groups} searchLabel={t('composer.model.find')} above={choice.state} below={options}
      onChoose={(row) => {
        if (row.id === DEFAULT_ROW) { if (!choice.disabled) choice.onChange(null); show(false); return; }
        const entry = choice.entries.find((candidate) => candidate.id === row.id);
        if (entry !== undefined) choose(entry);
        show(false);
      }} />
    {efforts.length > 0 && selected !== undefined && <ChoiceChip what={t('composer.effort.title')} value={effort === null ? '' : effortDisplayName(effort, t)} control="effort" disabled={choice.disabled === true}
      open={effortOpen} onOpenChange={setEffortOpen} groups={effortGroups(selected.option, effort, t)}
      onChoose={(row) => { choose(selected, row.id); setEffortOpen(false); }} />}
  </span>;
}

/** The controls of a thread's model, or of a model chosen on its own entries; persistence and native reads stay with the caller. */
export function ModelEffort(props: ModelEffortProps | ControlledModelEffortProps) {
  if ('choice' in props) return <ControlledModelEffort {...props} />;
  return <span className={cx('inline-flex min-w-0 max-w-full flex-wrap items-center gap-1')} data-model-controls><ThreadRunsChips thread={props.thread} readOnly={props.readOnly === true}
    {...(props.open === undefined ? {} : { open: props.open })} {...(props.onOpenChange === undefined ? {} : { onOpenChange: props.onOpenChange })} /></span>;
}
