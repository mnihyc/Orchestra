/** The summary targets, one compact row for each kind of thing that is summarized: whether it is refreshed automatically, and when, how often and how much.
 *
 * What each shows as its default comes from the registry's own default of the setting (what the engine falls back to as well), never from this page.
 * A target that has no entry of its own here inherits; the first change to it writes an entry that starts from what applied before.
 */
import type { TFunction } from 'i18next';
import { useQuery } from '@tanstack/react-query';
import { RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { targetSettingsQuery, type SettingsTarget } from '@/api/settings';

import { isRecord, SUMMARY_TARGETS, TARGET_NUMBERS, targetProblems, type SummaryTarget, type TargetNumber } from '@/features/settings/draftChecks';
import { useDraftFault } from '@/features/settings/drafts';
import { words } from '@/features/settings/settingsModel';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { settingChoice } from '@/i18n/choices';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';

/** The events a refresh may wait for, in the order the registry names them. */
/** The setting whose choices name the events. */
const TARGETS_SETTING = 'summary.targets';
const EVENTS = ['turn_completed', 'milestone', 'interval', 'content_changed', 'message_posted', 'child_changed'] as const;

interface Entry { automatic?: unknown; on?: unknown; min_interval_secs?: unknown; context_tokens?: unknown; output_chars?: unknown }

function entryOf(value: unknown, target: string): Entry | undefined {
  const entry = isRecord(value) ? value[target] : undefined;
  return isRecord(entry) ? entry : undefined;
}
const chosenEvents = (entry: Entry | undefined): string[] | undefined => Array.isArray(entry?.on) ? entry.on.filter((event): event is string => typeof event === 'string') : undefined;

export interface SummaryTargetsEditorProps extends ValueControlProps {
  /** The setting's default in the registry: what every target starts from. */
  defaults: unknown;
  /** The entries that places above set, which a target without an entry of its own follows. */
  inherited: unknown;
}

/** The same shared-trigger and producer fallbacks the summary engine uses for an omitted member. */
export function SummaryTargetsSetting({ target, defaults, ...props }: SummaryTargetsEditorProps & { target: SettingsTarget }) {
  const settings = useQuery(targetSettingsQuery(target));
  const triggers = settings.data?.find((row) => row.key === 'auto.triggers');
  const shared = isRecord(triggers?.value) ? triggers.value : {};
  const producer = settings.data?.find((row) => row.key === 'auto.summarizer')?.value;
  const context = isRecord(producer) ? numberOf(producer.context_tokens) : undefined;
  const effective = Object.fromEntries(SUMMARY_TARGETS.map((kind) => {
    const shipped = entryOf(defaults, kind);
    const interval = kind === 'thread' || triggers !== undefined && triggers.source.scope !== 'default' ? numberOf(shared.min_interval_secs) : undefined;
    return [kind, { ...shipped,
      ...(interval === undefined ? {} : { min_interval_secs: interval }),
      ...(kind === 'thread' && Array.isArray(shared.on) ? { on: shared.on } : {}),
      ...(context === undefined ? {} : { context_tokens: context }),
    }];
  }));
  return <SummaryTargetsEditor {...props} defaults={effective} />;
}

/** Whether an entry says nothing that the shipped default does not already say. */
function saysNothing(entry: Entry, shipped: Entry | undefined, inherited: Entry | undefined): boolean {
  return inherited === undefined && Object.keys(entry).length === 1 && entry.automatic === (shipped?.automatic === true);
}

export function SummaryTargetsEditor({ value, onChange, onEdited, disabled = false, defaults, inherited }: SummaryTargetsEditorProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? 'en';
  const held = isRecord(value) ? value : {};
  const problems = targetProblems(held);
  const change = (target: SummaryTarget, next: Entry | undefined) => {
    const { [target]: _before, ...rest } = held;
    onEdited?.();
    onChange(next === undefined ? rest : { ...rest, [target]: next });
  };
  return <ul aria-label={t('settingsEditors.targets.label')} className="flex min-w-0 flex-col divide-y divide-border rounded-md border border-border">
    {SUMMARY_TARGETS.map((target) => <TargetRow key={target} target={target} own={entryOf(held, target)} above={entryOf(inherited, target)} shipped={entryOf(defaults, target)}
      locale={locale} disabled={disabled} wrong={problems.filter((problem) => problem.target === target).map((problem) => problem.member)}
      onChange={(next) => change(target, next)} t={t} />)}
  </ul>;
}

function TargetRow({ target, own, above, shipped, locale, disabled, wrong, onChange, t }: {
  target: SummaryTarget; own: Entry | undefined; above: Entry | undefined; shipped: Entry | undefined; locale: string; disabled: boolean; wrong: readonly string[];
  onChange: (next: Entry | undefined) => void; t: TFunction;
}) {
  const name = t(`summaryOptions.fields.${target}`);
  const shown = own ?? above;
  const automatic = shown?.automatic === true || shown === undefined && shipped?.automatic === true;
  // What a member is when this entry does not say: an entry of its own replaces the one above as a whole, so only the shipped default is left under it.
  const under = own === undefined ? above : undefined;
  const base: Entry = own ?? { ...(above ?? {}), automatic };
  const write = (patch: Entry) => {
    // A member the patch gives as undefined is taken out, so the entry stops saying it.
    const next = Object.fromEntries(Object.entries({ ...base, ...patch }).filter(([, value]) => value !== undefined)) as Entry;
    onChange(saysNothing(next, shipped, above) ? undefined : next);
  };
  const events = chosenEvents(shown) ?? chosenEvents(shipped) ?? [];
  const ownEvents = chosenEvents(own);
  return <li data-summary-target={target} className="flex min-w-0 flex-col gap-2 px-3 py-2">
    <div className="flex min-w-0 items-center justify-between gap-2">
      <span className="min-w-0 font-medium wrap-anywhere">{name}</span>
      <div className="flex shrink-0 items-center gap-1">
        {own !== undefined && <IconButton size="sm" label={t('settingsEditors.targets.useDefault', { name })} icon={<RotateCcw aria-hidden />} disabled={disabled} onClick={() => onChange(undefined)} />}
        <Switch className="w-fit items-center gap-2" disabled={disabled} checked={automatic} onCheckedChange={(next) => write({ automatic: next })}
          aria-label={`${name}: ${t('settingsEditors.targets.automatic')}`} label={t('settingsEditors.targets.automatic')} />
      </div>
    </div>
    {automatic && <div className="flex min-w-0 flex-col gap-2">
      <div role="group" aria-label={t('settingsEditors.targets.whenFor', { name })} className="flex min-w-0 flex-wrap items-center gap-1">
        <span className="pr-1 text-sm text-text-muted">{t('settingsEditors.targets.when')}</span>
        {EVENTS.map((event) => {
          const on = events.includes(event);
          const eventName = settingChoice(t, TARGETS_SETTING, event)?.name ?? words(event);
          return <button key={event} type="button" aria-pressed={on} disabled={disabled} data-event={event}
            onClick={() => write({ on: EVENTS.filter((other) => other === event ? !on : events.includes(other)) })}
            className={cx('focus-inset inline-flex h-(--control-sm) items-center rounded-full border px-3 text-sm transition-colors disabled:cursor-not-allowed disabled:text-text-disabled',
              on ? 'border-accent bg-accent-subtle font-medium text-text' : 'border-control-border bg-surface text-text-muted hover:bg-bg-muted hover:text-text')}>{eventName}</button>;
        })}
        {ownEvents !== undefined && <button type="button" disabled={disabled} className="focus-inset rounded-sm px-2 text-sm text-text-muted underline underline-offset-2 hover:text-text" onClick={() => write({ on: undefined })}>{t('settingsEditors.targets.defaultEvents')}</button>}
      </div>
      <div className="grid min-w-0 grid-cols-1 gap-2 @min-[34rem]:grid-cols-3">
        {TARGET_NUMBERS.map(([member, least]) => <NumberField key={member} target={target} member={member} least={least} own={own} placeholder={numberOf(under?.[member]) ?? numberOf(shipped?.[member])}
          locale={locale} disabled={disabled} wrong={wrong.includes(member)} onChange={(next) => write(patchOf(member, next))} t={t} />)}
      </div>
    </div>}
  </li>;
}

const numberOf = (value: unknown): number | undefined => typeof value === 'number' ? value : undefined;

/** The change of one number of an entry; no number takes the member out, so that the default applies. */
function patchOf(member: TargetNumber, next: number | undefined): Entry {
  return member === 'min_interval_secs' ? { min_interval_secs: next } : member === 'context_tokens' ? { context_tokens: next } : { output_chars: next };
}

function NumberField({ target, member, least, own, placeholder, locale, disabled, wrong, onChange, t }: {
  target: SummaryTarget; member: TargetNumber; least: number; own: Entry | undefined; placeholder: number | undefined; locale: string; disabled: boolean; wrong: boolean;
  onChange: (next: number | undefined) => void; t: TFunction;
}) {
  const given = own?.[member];
  const fault = useDraftFault(`${target}.${member}`);
  return <Input type="number" inputMode="numeric" min={least} step={1} label={t(`settingsEditors.targets.${member}`)} disabled={disabled}
    value={typeof given === 'number' ? given : ''} placeholder={placeholder === undefined ? undefined : formatNumber(placeholder, locale)}
    error={wrong ? t('settingsEditors.targets.least', { least: formatNumber(least, locale) }) : fault}
    onChange={(event) => {
      const text = event.target.value.trim();
      if (text === '') onChange(undefined);
      else if (/^-?\d+$/u.test(text)) onChange(Number(text));
    }} />;
}
