/** Model, by its display name, and reasoning effort chosen together at the composer; applies from the next turn.
 *
 * The model and its effort, chosen together in one place. A compact control names the model with its
 * selected options in parentheses; it opens full-width model and effort rows, each checked where selected, and
 * typing in the list finds a model by either of its names. A model is shown by its display name.
 * Under the list stand the choices that this
 * model has open and no other: Speed, Access and Context, each only where the model's managers switched it
 * on at the provider, each opening on what the thread runs with now and saying where that comes from. The
 * control shows the thread's own state and never what the provider allows, which is on the provider's
 * Models tab. A change applies from the next turn, or as the choice says. Changing a model drops a choice
 * the new one does not have, with a line saying so. Changing any of it is setting the session up:
 * somebody who only drives the thread sees it read-only, with who can change it. Models and efforts that
 * are not allowed are listed below, with the reason.
 */
import { useCompactControls } from '@/features/workbench/Narrow';
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { Check, ChevronDown, Lock } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ContextChoice } from '@/features/composer/ContextChoice';
import { type Dropped, forceLabel, type Kind, type State, speedStart, programStart } from '@/features/composer/modelState';
import { useChoicePerformance } from '@/features/usage/PerformancePlaces';
import { ResourceHint, ResourceName, resourcePreview } from '@/features/resource-facts/ResourceFacts';
import { modelResource } from '@/features/resource-facts/facts';
import { changeModelChoice, isSettingConflict, useRunsWith } from '@/features/composer/runsWith';
import { SettingConflict } from '@/features/composer/SettingConflict';
import { SettingHelpGroup } from '@/features/help/SettingHelpGroup';
import { AccessChoice, SpeedChoice } from '@/features/composer/SpeedAndAccess';
import { formatContext } from '@/features/models/context';
import { accessProgramDisplayName, effortDisplayName, modelDisplayName } from '@/features/models/names';
import type { ModelOption, Thread } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { Button } from '@/ui/Button';
import { Choice } from '@/ui/Choice';
import { ChoiceCard, ChoiceInspection, type CardPreview } from '@/ui/DetailCard';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';

export interface ModelEffortProps {
  thread: Thread;
  /** The composer's compact control, or the chip of the header with the agent's name and the context ring. */
  variant: 'composer' | 'chip';
  /** Where the control is opened and closed from outside: by a command of the box. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** The choice the keyboard goes to when the control opens. */
  section?: Kind | null;
  /** Fits the composer's phone grid without hiding its delivery controls. */
  compact?: boolean;
  /** A watch-only row never changes a parameter, even when another permission could edit it. */
  readOnly?: boolean;
}

/** An unset model stays empty; a missing display name never turns a routing key into a title. */
export const modelName = (thread: Pick<Thread, 'model' | 'model_display'>): string => thread.model === null ? '' : modelDisplayName(thread.model_display);

/** A choice's wire identity is separate from its visible name. */
const optionName = (option: Pick<ModelOption, 'display_name' | 'model_name'>): string => modelDisplayName(option.display_name);

/** The menu keeps one tab stop; moving focus never writes a model or effort. */
function moveChoiceFocus(event: KeyboardEvent<HTMLUListElement>) {
  if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || isComposing(event.nativeEvent)
    || !['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) return;
  const rows = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)'));
  if (rows.length === 0) return;
  const target = event.target instanceof Node ? event.target : null;
  const at = rows.findIndex((row) => row.contains(target));
  const next = event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1
    : at < 0 ? event.key === 'ArrowUp' ? rows.length - 1 : 0 : (at + (event.key === 'ArrowDown' ? 1 : -1) + rows.length) % rows.length;
  event.preventDefault(); event.stopPropagation(); rows[next]?.focus();
}

const choiceKey = (model: string, effort: string | null = null): string => JSON.stringify([model, effort]);

/** Model and effort use the same borderless picker row and one checked mark. */
function PickerRow({ name, description, preview, checked, disabled, tabStop, nested = false, onFocused, onChoose }: {
  name: string; description: ReactNode; preview?: CardPreview; checked: boolean; disabled: boolean; tabStop: boolean; nested?: boolean;
  onFocused: () => void; onChoose: () => void;
}) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  return <Choice presentation="picker" compact name={preview?.renderName === undefined ? name : preview.renderName(name)} description={description} className={cx('w-full', disabled ? 'text-text-disabled' : 'text-text')}>
    {(content, control) => <ChoiceCard preview={preview}><button {...control} type="button" role="menuitemradio" aria-checked={checked} disabled={disabled}
      tabIndex={!disabled && tabStop ? 0 : -1} data-highlighted={!disabled && (hovered || focused) ? '' : undefined}
      onFocus={() => { setFocused(true); onFocused(); }} onBlur={() => setFocused(false)}
      onPointerEnter={() => setHovered(true)} onPointerLeave={() => setHovered(false)} onClick={onChoose}
      className={cx('choice-option flex min-h-8 min-w-0 w-full items-center gap-2 rounded-sm px-2 py-1 text-left text-sm disabled:cursor-default', nested && 'pl-8')}>
      <span aria-hidden className="flex size-4 shrink-0 items-center justify-center text-accent-text">{checked && <Check aria-hidden />}</span>
      {content}
    </button></ChoiceCard>}
  </Choice>;
}

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

function Choices({ thread, section }: { thread: Thread; section: Kind | null }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [filter, setFilter] = useState('');
  const [notice, setNotice] = useState<string[]>([]);
  const [focusKey, setFocusKey] = useState<string | null>(null);
  const menu = useRef<HTMLUListElement>(null);
  const runs = useRunsWith(thread, true);
  const performance = useChoicePerformance(thread);
  const mayChange = holds(thread.access, 'thread.manage');
  // The list is short where choices stand beneath it: it opens at the model the thread runs, not at its first.
  const currentRow = useRef<HTMLLIElement>(null);
  const model = runs.parameters?.model.value ?? null;
  useEffect(() => {
    currentRow.current?.scrollIntoView({ block: 'nearest' });
  }, [model, runs.pending]);
  // Opened by a command of the box, the keyboard goes to the choice that was asked for, after the card has taken it first.
  const card = useRef<HTMLDivElement>(null);
  const ready = !runs.pending && runs.parameters !== undefined;
  useEffect(() => {
    if (section === null || !ready) return;
    const timer = setTimeout(() => {
      const target = card.current?.querySelector<HTMLElement>(`[data-section="${section}"] [role=radio][aria-checked=true], [data-section="${section}"] [role=combobox], [data-section="${section}"] button, [data-section="${section}"] input`);
      target?.focus();
    }, 0);
    return () => clearTimeout(timer);
  }, [section, ready]);

  const change = useMutation({
    mutationFn: (choice: { model: string; effort: string | null; option: ModelOption }) => changeModelChoice(thread, choice, runs.states, runs.settings ?? []),
    onMutate: runs.clearRefusal,
    onError: async (error) => {
      await runs.refuse(error);
      if (isSettingConflict(error)) setFilter('');
    },
    onSuccess: ({ dropped, option }) => {
      const lines = dropped.map((one) => leftBehind(one, option, locale, t));
      setNotice(lines);
      if (lines.length > 0) toast({ tone: 'info', title: t('composer.model.dropped.title'), description: lines.join(' ') });
    },
  });

  if (runs.pending) return <SkeletonRows rows={3} />;
  if (runs.parameters === undefined) return <ErrorAlert error={runs.failed} />;
  const parameters = runs.parameters;
  const needle = filter.trim().toLowerCase();
  const matches = (option: ModelOption) => needle === '' || option.display_name.toLowerCase().includes(needle) || option.model_name.toLowerCase().includes(needle);
  const allowed = parameters.model.options.filter((option) => option.allowed && matches(option));
  const refused = parameters.model.options.filter((option) => !option.allowed && matches(option));
  const writable = mayChange && parameters.model.writable && runs.ready && !change.isPending;
  const effortSupported = parameters.effort.supported;
  const visibleEffort = effortSupported && allowed.some((option) => (option.parameters.efforts?.length ?? 0) > 0);
  const current = parameters.model.options.find((option) => option.model_name === parameters.model.value);
  const keys = allowed.flatMap((option) => [choiceKey(option.model_name), ...(effortSupported ? (option.parameters.efforts ?? []).map((effort) => choiceKey(option.model_name, effort)) : [])]);
  const initial = allowed.find((option) => option.model_name === model) ?? allowed[0];
  const tabStop = focusKey !== null && keys.includes(focusKey) ? focusKey : initial === undefined ? null : choiceKey(initial.model_name);
  const states = runs.states;
  const helpSettings = ['agent.model', ...(visibleEffort ? ['agent.effort'] : []), ...(states.length > 0 ? ['agent.params'] : [])];
  // A choice is written by whoever may set the session up, and only while nothing else is being written.
  const may = mayChange && runs.ready && !change.isPending && !runs.choosing;
  const rate = current?.parameters.rate ?? null;
  const tier = states.find((state) => state.kind === 'speed')?.force;
  const program = states.find((state) => state.kind === 'access')?.force;
  const hint = (option: ModelOption, effort: string | null) => performance(option.model_name, effort, option.model_name === model && typeof tier === 'string' ? tier : speedStart(option.parameters), option.model_name === model && typeof program === 'string' ? program : programStart(option.parameters));

  return (
    <div ref={card} className="flex min-h-0 w-80 max-w-full flex-col gap-3">
      <div className="flex min-h-0 flex-col gap-2">
        <div className="flex items-center gap-2"><Input label={t('composer.model.find')} hideLabel placeholder={t('composer.model.find')} value={filter} onChange={(event) => setFilter(event.target.value)}
          onKeyDown={(event) => {
            if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || isComposing(event.nativeEvent) || !['ArrowDown', 'ArrowUp'].includes(event.key)) return;
            const rows = Array.from(menu.current?.querySelectorAll<HTMLButtonElement>('[role="menuitemradio"]:not(:disabled)') ?? []);
            const target = event.key === 'ArrowUp' ? rows.at(-1) : rows.find((row) => row.tabIndex === 0) ?? rows[0];
            if (target !== undefined) { event.preventDefault(); event.stopPropagation(); target.focus(); }
          }} /><SettingHelpGroup settingKeys={helpSettings} target={{ kind: 'node', id: thread.id }} term={t(visibleEffort ? 'helpGroups.modelAndEffort' : 'helpGroups.model')} article="resources" heading="providers-and-models" /></div>
        {!mayChange && <p className="text-sm text-text-muted">{t('composer.model.readOnly', { who: thread.owner.display_name })}</p>}
        {!effortSupported && <p className="text-sm text-text-muted">{t('composer.model.noEffort')}</p>}
        <ChoiceInspection><ul
          ref={menu}
          role="menu"
          aria-orientation="vertical"
          onKeyDown={moveChoiceFocus}
          className={cx('flex min-h-0 w-full flex-col gap-1 overflow-y-auto', states.length > 0 ? 'max-h-(--list-max-height-short)' : 'max-h-(--list-max-height)')}
          aria-label={t('composer.model.list')}
          // Where every model in it is switched off for this reader there is nothing in it to take the keyboard, and a list that scrolls has to be reached.
          {...(writable ? {} : { tabIndex: 0 })}
        >
          {allowed.map((option) => {
            const efforts = option.parameters.efforts ?? [];
            const isCurrent = option.model_name === parameters.model.value;
            const subject = modelResource(option, thread.binding?.provider.id ?? '', { usage: parameters.provider?.choice_usage, effort: isCurrent ? parameters.effort.value : option.parameters.default_effort, speed: hint(option, isCurrent ? parameters.effort.value : option.parameters.default_effort ?? null), impact: t('resourceFacts.nextTurn') });
            return (
              <li key={option.model_name} ref={isCurrent ? currentRow : undefined} role="none" className="flex w-full flex-col">
                <PickerRow name={optionName(option)} description={<ResourceHint subject={subject} />} preview={resourcePreview(subject)}
                  checked={isCurrent} disabled={!writable} tabStop={tabStop === choiceKey(option.model_name)} onFocused={() => setFocusKey(choiceKey(option.model_name))}
                  onChoose={() => change.mutate({ model: option.model_name, effort: option.parameters.default_effort ?? null, option })} />
                {efforts.length > 0 && effortSupported && (
                  <div role="group" aria-label={t('composer.model.efforts', { model: optionName(option) })} className="flex w-full flex-col">
                    {efforts.map((effort) => (
                      <PickerRow key={effort} nested name={effortDisplayName(effort, t)} description={<ResourceHint subject={{ ...subject, effort, speed: hint(option, effort) }} />} preview={resourcePreview({ ...subject, effort, speed: hint(option, effort) })}
                        checked={isCurrent && parameters.effort.value === effort} disabled={!writable} tabStop={tabStop === choiceKey(option.model_name, effort)}
                        onFocused={() => setFocusKey(choiceKey(option.model_name, effort))}
                        onChoose={() => change.mutate({ model: option.model_name, effort, option })} />
                    ))}
                  </div>
                )}
              </li>
            );
          })}
        </ul></ChoiceInspection>
        {notice.length > 0 && (
          <InlineAlert tone="info" title={t('composer.model.dropped.title')}>
            <ul className="flex flex-col gap-1">
              {notice.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          </InlineAlert>
        )}
        <SettingConflict refusal={runs.refusal} />
        {change.error !== null && runs.refusal === null && <ErrorAlert error={change.error} />}
      </div>

      {states.length > 0 && (
        <div role="group" aria-label={t('composer.model.runsWith')} className="flex flex-col gap-3 border-t border-border pt-3" data-runs-with-group>
          {states.map((state) => {
            const chooser = runs.chooser(state.key);
            const free = may && state.writable;
            if (state.kind === 'context') return <ContextChoice key={state.key} state={state} writable={free} chooser={chooser} onChoose={(size) => runs.choose(state, size)} />;
            if (state.kind === 'speed') return <SpeedChoice key={state.key} state={state} writable={free} chooser={chooser} rate={rate} performance={(value) => model === null ? null : performance(model, parameters.effort.value, value, typeof program === 'string' ? program : null, true)} onChoose={(value) => runs.choose(state, value)} />;
            return <AccessChoice key={state.key} state={state} writable={free} chooser={chooser} reopen={runs.refusal?.member === state.key} performance={(value) => model === null ? null : performance(model, parameters.effort.value, typeof tier === 'string' ? tier : null, value, true)} onChoose={(value) => runs.choose(state, value)} />;
          })}
          {runs.chooseError !== null && runs.refusal === null && <ErrorAlert error={runs.chooseError} />}
        </div>
      )}

      {refused.length > 0 && (
        <div className="flex flex-col gap-1 border-t border-border pt-2">
          <p className="text-xs font-medium text-text-subtle">{t('composer.model.notAllowed')}</p>
          <ul className="flex flex-col gap-1">
            {refused.map((option) => (
              <li key={option.model_name} className="flex flex-col text-sm text-text-muted">
                <Choice compact name={optionName(option)} description={t(`composer.model.reason.${option.reason ?? 'disabled'}`, { provider: thread.binding?.provider.name ?? '' })} />
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}

export function ModelEffort({ thread, variant, open, onOpenChange, section = null, compact = false, readOnly = false }: ModelEffortProps) {
  const narrow = useCompactControls();
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const runs = useRunsWith(thread, false);
  const current = runs.parameters?.model.options.find((option) => option.model_name === thread.model);
  const subject = current === undefined || thread.binding === null ? null : { kind: 'model' as const, model: current, providerId: thread.binding.provider.id, usage: runs.parameters?.provider?.choice_usage, effort: thread.effort, impact: t('resourceFacts.nextTurn') };
  const name = modelName(thread);
  const options = [...(thread.effort === null || name === '' ? [] : [effortDisplayName(thread.effort, t)]), ...runsWithParts(runs.states, locale)];
  const label = options.length === 0 ? name : `${name} (${options.join(t('startFlow.separator'))})`;
  // How full the context is stands beside this chip, as its own ring that opens what fills it.
  const bound = thread.binding !== null;
  const mayManage = holds(thread.access, 'thread.manage');
  const trigger =
    variant === 'composer' ? (
      <Button action="models" variant="ghost" size="sm" disabled={!bound} aria-label={t('composer.model.change', { model: label === '' ? t('composer.model.none') : label })} className={compact ? cx('w-full min-w-0 max-w-full', narrow ? 'gap-1 px-1 text-xs [&>svg]:hidden' : 'gap-2 px-2 text-sm') : 'max-w-(--chip-width-wide)'}>
        {compact ? <><span className={cx('min-w-0 truncate text-left', !narrow && 'hidden')}>{name === '' ? t('composer.model.none') : name}</span><span className={cx('min-w-0 truncate', narrow ? 'hidden' : 'inline')}>{label === '' ? t('composer.model.none') : label}</span></> : <span className="truncate">{label === '' ? t('composer.model.none') : label}</span>}
        <ChevronDown aria-hidden className="shrink-0" />
      </Button>
    ) : (
      <Button action="models" variant="ghost" size="sm" disabled={!bound} aria-label={t('composer.model.change', { model: label === '' ? t('composer.model.none') : label })} className="max-w-(--chip-width)">
        <span className="truncate text-text-muted">{label === '' ? t('composer.model.none') : label}</span>
        {!mayManage && <Lock className="text-text-subtle" aria-hidden />}
      </Button>
    );
  return (
    <span className="inline-flex min-w-0 max-w-full items-center gap-1"><Popover scroll="children"
      label={t('composer.model.title')}
      trigger={<Tooltip content={label === '' ? undefined : label}>{trigger}</Tooltip>}
      side={variant === 'composer' ? 'top' : 'bottom'}
      className="max-w-[min(20rem,calc(100vw-2rem))]"
      {...(open === undefined ? {} : { open })}
      {...(onOpenChange === undefined ? {} : { onOpenChange })}
    >
      {readOnly ? <p className="text-sm text-text-muted">{t('composer.row.readOnly')}</p> : <Choices thread={thread} section={section} />}
    </Popover>{subject?.model.parameters.context_window !== undefined && subject.model.parameters.context_window !== null && <ResourceName subject={subject} hint={false} className={compact ? 'hidden @sm/composer:inline-flex' : undefined}><ResourceHint subject={subject} className="truncate" /></ResourceName>}</span>
  );
}
