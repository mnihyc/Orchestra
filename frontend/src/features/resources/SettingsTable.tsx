/** The settings a resource resolves, each with its value and where that value comes from, and whether the reader may change it.
 */
import { useTranslation } from 'react-i18next';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import type { ReactNode } from 'react';

import { SettingRow, type RowPlace } from '@/features/settings/SettingRow';
import { localizedSetting } from '@/features/settings/settingText';
import { lockAbove, withGroup } from '@/features/settings/settingsModel';
import { ReadOnlySettingsNotice } from '@/features/settings/ReadOnlyNotice';
import { runtimeEffect } from '@/features/settings/runtimeEffects';
import { registryQuery, targetSettingsKey, type SettingsTarget } from '@/api/settings';
import type { ResolvedSetting, RuntimeChanges } from '@/generated/api-types';
import { SettingRow as SettingRowLayout } from '@/ui/SettingRow';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { SettingSummary } from '@/features/settings/SettingSummary';

/** A value as it is read: words stay as they are, a list is joined, anything else is written as JSON. */
export function settingValue(value: unknown): string {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  if (Array.isArray(value) && value.every((entry) => typeof entry === 'string' || typeof entry === 'number')) return value.join(', ');
  return JSON.stringify(value);
}

export function SettingsTable({ settings, label, target, place, runtime, effectFor, readOnlyNotice = true }: { settings: readonly ResolvedSetting[]; label: string; target?: SettingsTarget; place?: RowPlace; runtime?: RuntimeChanges | null; effectFor?: (key: string) => ReactNode; readOnlyNotice?: boolean }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const registry = useQuery({ ...registryQuery, enabled: target !== undefined });
  if (target !== undefined && registry.isPending) return <SkeletonRows rows={3} />;
  if (target !== undefined && registry.isError) return <ErrorState compact error={registry.error} onRetry={() => void registry.refetch()} />;
  const definitions = new Map((registry.data ?? []).map((def) => [def.key, withGroup(localizedSetting(def, i18n, i18n.resolvedLanguage))]));
  const readOnly = settings.some((row) => !row.writable || target !== undefined && lockAbove(row, target) !== null);
  const editable = target !== undefined && settings.some((row) => row.writable && lockAbove(row, target) === null);
  return <>{readOnlyNotice && readOnly && <ReadOnlySettingsNotice owner={place?.owner} partial={editable} />}<ul aria-label={label} className="min-w-0 divide-y divide-border rounded-lg border border-border bg-surface">{settings.map((setting) => {
    const def = definitions.get(setting.key);
    const effect = runtimeEffect(setting.key, runtime);
    const note = effectFor === undefined ? effect === null ? null : <span className="text-xs text-text-muted">{t(`runtimeChanges.${effect}`)}</span> : effectFor(setting.key);
    if (def === undefined || target === undefined) {
      return <SettingRowLayout key={setting.key} name={def?.title ?? t('settings.compact.definitionMissing')} description={def?.description} settingKey={setting.key}
        value={<SettingSummary def={def ?? { key: setting.key }} value={setting.value} />} readOnly
        reason={t(def === undefined ? 'settings.compact.definitionMissingReason' : 'settings.compact.readOnly')}
        actions={def === undefined && target !== undefined ? <Button size="sm" pending={registry.isFetching} onClick={() => void registry.refetch()}>{t('common.tryAgain')}</Button> : undefined} />;
    }
    const context = place ?? { name: label, kind: null, owner: null, access: null };
    return <SettingRow key={setting.key} def={def} setting={setting} target={target} place={context} effectNote={note}
      onChanged={(next) => client.setQueryData<ResolvedSetting[]>(targetSettingsKey(target), (before) => before?.map((row) => row.key === next.key ? next : row))} />;
  })}</ul></>;
}
