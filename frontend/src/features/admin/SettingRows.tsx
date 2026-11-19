/** Plain settings of the system, each changed where it is shown: a switch, a choice or a number, with where its value comes from and the way back to what Orch ships.
 *
 * Several screens of Management show a setting as a fact (when a host counts as full, who may
 * register, whether a password signs people in) and change it where it is shown. Each row here reads its
 * setting from the system's own, writes it with one request that is shown as under way until the
 * server has answered, and says whether the value is Orch's or was set here, with the way to put it back.
 * A changed control is a draft until its answer; refused drafts remain visible with their error.
 */
import { useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { settingChoice } from '@/i18n/choices';

import { settingsKey, systemSettingsQuery } from '@/api/admin';
import { clearSetting, putSetting, targetSettingsKey } from '@/api/settings';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import type { ResolvedSetting } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select, type SelectOption } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Switch } from '@/ui/Switch';

/** The system's settings as a screen reads them, and the one it wants. */
export function useSystemSettings() {
  const settings = useQuery(systemSettingsQuery);
  const find = (key: string): ResolvedSetting | undefined => settings.data?.find((one) => one.key === key);
  return { settings, find };
}

function ownValue(setting: ResolvedSetting | undefined): unknown {
  const own = setting?.own;
  return own === null || own === undefined ? setting?.value : own.value;
}

/** Writes and clears one setting; the answer replaces the row, so the screen says what the server holds. */
export function useSettingChange(key: string) {
  const { find, settings } = useSystemSettings();
  const setting = find(key);
  const cache = useQueryClient();
  const auto = useAutoSave({ current: { value: { value: ownValue(setting), locked: setting?.own?.locked === true, own: setting?.own !== null && setting?.own !== undefined }, revision: setting?.own?.revision ?? null },
    enabled: setting?.writable === true, reload: () => settings.refetch({ throwOnError: true }),
    write: async (value, before) => {
      const accepted = value.own ? await putSetting({ kind: 'system' }, key, { value: value.value, locked: value.locked, expectedRevision: typeof before.revision === 'number' ? before.revision : null })
        : await clearSetting({ kind: 'system' }, key, typeof before.revision === 'number' ? before.revision : null);
      acceptSystemSetting(cache, accepted);
      return { value: { value: ownValue(accepted), locked: accepted.own?.locked === true, own: accepted.own !== null }, revision: accepted.own?.revision ?? null };
    } });
  return {
    save: { isPending: auto.pending, variables: auto.value, mutate: (change: { value: unknown; locked?: boolean }) => auto.choose({ value: change.value, locked: change.locked ?? setting?.own?.locked === true, own: true }) },
    reset: { mutate: () => auto.choose({ ...auto.value, own: false }) },
    pending: auto.pending, error: auto.error, writable: setting?.writable === true, auto,
  };
}

function acceptSystemSetting(cache: QueryClient, accepted: ResolvedSetting) {
  for (const key of [settingsKey, targetSettingsKey({ kind: 'system' })]) cache.setQueryData<ResolvedSetting[]>(key, (held) => held?.map((one) => one.key === accepted.key ? accepted : one));
}

export function SettingChangeStatus({ change, name }: { change: ReturnType<typeof useSettingChange>; name?: string | undefined }) { return <AutoSaveFeedback save={change.auto} name={name} />; }

/** Where a value comes from: what Orch ships, or what was set here, with the way back to the first. */
export function Provenance({ setting, onReset, pending }: { setting: ResolvedSetting; onReset: () => void; pending: boolean }) {
  const { t } = useTranslation();
  const own = setting.own !== null;
  return (
    <span className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
      <span>{own ? t('admin.settingRow.setHere') : t('admin.settingRow.shipped')}</span>
      {own && (
        <Button size="sm" variant="ghost" disabled={pending} onClick={onReset}>
          {t('admin.settingRow.putBack')}
        </Button>
      )}
    </span>
  );
}

interface RowProps {
  settingKey: string;
  label: ReactNode;
  hint?: ReactNode;
  /** Why the setting cannot be changed now; the control is then off and says so. */
  off?: ReactNode;
  className?: string;
}

/** A row's frame: the control, what is wrong with the last request, and where the value comes from. */
function Row({ setting, change, children, className, name }: { setting: ResolvedSetting; change: ReturnType<typeof useSettingChange>; children: ReactNode; className?: string; name?: string | undefined }) {
  return (
    <div data-item data-setting={setting.key} className={cx('flex flex-col gap-1', className)}>
      {children}
      <div className="flex flex-wrap items-center gap-x-3">
        <Provenance setting={setting} pending={change.pending || !change.writable} onReset={() => change.reset.mutate()} />
      </div>
      <SettingChangeStatus change={change} name={name} />
    </div>
  );
}

export interface SwitchSettingProps extends RowProps {
  /** What the setting holds when the switch is off and when it is on, for a setting that is a word and not a yes or a no. */
  words?: readonly [off: unknown, on: unknown];
}

/** A setting that is on or off. */
export function SwitchSetting({ settingKey, label, hint, off, words = [false, true], className }: SwitchSettingProps) {
  const { find, settings } = useSystemSettings();
  const change = useSettingChange(settingKey);
  const setting = find(settingKey);
  if (setting === undefined) return settings.isPending ? <SkeletonRows rows={1} /> : null;
  return (
    <Row setting={setting} change={change} className={className} name={typeof label === 'string' ? label : undefined}>
      <Switch
        presentation="compact"
        label={label}
        hint={off ?? hint}
        // The chosen draft stays visible through a refusal; only an accepted answer offers Undo.
        checked={change.save.variables.value === words[1]}
        disabled={change.pending || !change.writable || (off !== undefined && off !== false)}
        onCheckedChange={(next) => change.save.mutate({ value: next ? words[1] : words[0] })}
      />
    </Row>
  );
}

export interface ChoiceSettingProps<Value extends string> extends RowProps {
  options: readonly SelectOption<Value>[];
}

/** A setting that is one of a few words. */
export function ChoiceSetting<Value extends string>({ settingKey, label, hint, options, off, className }: ChoiceSettingProps<Value>) {
  const { t } = useTranslation();
  const { find, settings } = useSystemSettings();
  const change = useSettingChange(settingKey);
  const setting = find(settingKey);
  if (setting === undefined) return settings.isPending ? <SkeletonRows rows={1} /> : null;
  return (
    <Row setting={setting} change={change} className={className} name={typeof label === 'string' ? label : undefined}>
      <Select
        label={label}
        hint={off ?? hint}
        value={String(change.save.variables.value) as Value}
        disabled={change.pending || !change.writable || (off !== undefined && off !== false)}
        options={options.map((option) => {
          const copy = settingChoice(t, settingKey, option.value);
          return copy === null ? option : { ...option, ...copy, description: option.description ?? copy.description };
        })}
        onValueChange={(next) => change.save.mutate({ value: next })}
        fieldClassName="max-w-(--container-form)"
      />
    </Row>
  );
}

export interface NumberSettingProps extends RowProps {
  min: number;
  max: number;
  /** What the number is counted in, written after the field. */
  unit?: string;
}

/** A whole-number setting saves a valid rested edit and retains an invalid draft. */
export function NumberSetting({ settingKey, label, hint, min, max, unit, off, className }: NumberSettingProps) {
  const { t } = useTranslation();
  const { find, settings } = useSystemSettings();
  const setting = find(settingKey);
  const cache = useQueryClient();
  const snapshot = (row: ResolvedSetting | undefined) => { const value = ownValue(row); return { text: typeof value === 'number' ? String(value) : '', locked: row?.own?.locked === true, own: row?.own !== null && row?.own !== undefined }; };
  const allowed = setting?.writable === true && (off === undefined || off === false);
  const valid = (value: ReturnType<typeof snapshot>) => !value.own || (value.text.trim() !== '' && Number.isInteger(Number(value.text)) && Number(value.text) >= min && Number(value.text) <= max);
  const save = useAutoSave({ current: { value: snapshot(setting), revision: setting?.own?.revision ?? null }, enabled: allowed, valid,
    reload: () => settings.refetch({ throwOnError: true }), write: async (value, before) => {
      const revision = typeof before.revision === 'number' ? before.revision : null;
      const accepted = value.own ? await putSetting({ kind: 'system' }, settingKey, { value: Number(value.text), locked: value.locked, expectedRevision: revision })
        : await clearSetting({ kind: 'system' }, settingKey, revision);
      acceptSystemSetting(cache, accepted); return { value: snapshot(accepted), revision: accepted.own?.revision ?? null };
    } });
  if (setting === undefined) return settings.isPending ? <SkeletonRows rows={1} /> : null;
  const fault = save.dirty && !valid(save.value) ? t('admin.settingRow.range', { min, max }) : undefined;
  return (
    <div data-item data-setting={settingKey} className={cx('flex flex-col gap-1', className)}>
      <form
        className="flex flex-col gap-1"
        onBlur={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) save.commit(); }}
        onCompositionStart={() => save.composing(true)} onCompositionEnd={() => save.composing(false)}
        onSubmit={(event) => {
          event.preventDefault();
          save.commit();
        }}
      >
        <div className="flex flex-wrap items-end gap-2">
          <Input
            label={label}
            type="number"
            inputMode="numeric"
            min={min}
            max={max}
            value={save.value.text}
            error={fault}
            disabled={!allowed || save.reloading}
            onChange={(event) => save.edit({ ...save.value, text: event.target.value, own: true })}
            fieldClassName="w-[10rem]"
            trailing={unit === undefined ? undefined : <span className="pr-2 text-sm text-text-muted">{unit}</span>}
          />
        </div>
        {(off ?? hint) !== undefined && <p className="max-w-prose text-sm text-text-muted">{off ?? hint}</p>}
      </form>
      <Provenance setting={setting} pending={save.pending || !allowed} onReset={() => save.choose({ ...save.value, own: false })} />
      <AutoSaveFeedback save={save} name={typeof label === 'string' ? label : undefined} />
    </div>
  );
}

/** A setting's state as a list entry. Used by screens that show a setting beside what it governs. */
export function SettingError({ query }: { query: ReturnType<typeof useSystemSettings>['settings'] }) {
  if (!query.isError) return null;
  return <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
}
