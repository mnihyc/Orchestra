/** Audit protocol names stay in requests and downloads; the trail speaks the catalogue's words. */
import type { TFunction } from 'i18next';
import type { AuditEntry, SettingDef } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { shapeOf } from '@/features/settings/settingsModel';
import { settingChoice } from '@/i18n/choices';
import { kindOf } from './catalogue';

/** Only registered actions are composed. A future action never leaks its wire name as a label. */
export function auditActionName(action: string, t: TFunction): string {
  if (kindOf(action) === undefined) return t('trace.unrecognizedAct');
  const [noun, verb] = action.split('.');
  const thing = t(`trace.actionThings.${noun ?? ''}`, { defaultValue: t('trace.object') });
  return t(`trace.actionVerbs.${verb ?? ''}`, { thing, defaultValue: t('trace.unrecognizedAct') });
}

/** The small historical value the server recorded, using the setting's own choice names. */
function settingValue(def: SettingDef, value: unknown, t: TFunction): string | null {
  if (value === undefined || value === null) return null;
  // A historical routing name has no historical display name alongside it. Do not invent one or fetch today's model.
  if (def.key === 'agent.model') return null;
  if (value === true || value === 'true') return t('trace.valueOn');
  if (value === false || value === 'false') return t('trace.valueOff');
  if (typeof value === 'string') return settingChoice(t, def.key, value)?.name ?? (shapeOf(def.schema).type === 'choice' ? t('trace.valueUnreported') : value);
  if (typeof value === 'number') return String(value);
  return t('trace.structuredValue');
}

export interface AuditSetting {
  title: string;
  value: string | null;
  was: string | null;
  reset: boolean;
  locked: boolean | null;
}

export function auditSetting(entry: Pick<AuditEntry, 'action' | 'details'>, t: TFunction): AuditSetting | null {
  if (!['settings.updated', 'settings.reset', 'system.settings_updated', 'settings.secret_set', 'settings.secret_deleted'].includes(entry.action)) return null;
  const details = entry.details;
  if (typeof details !== 'object' || details === null || !('key' in details) || typeof details.key !== 'string') return null;
  const def = SETTINGS_REGISTRY.find((row) => row.key === details.key);
  if (def === undefined) return null;
  return {
    title: t(`settingRegistry.${def.key}.title`, { defaultValue: def.title }),
    value: settingValue(def, 'value' in details ? details.value : undefined, t),
    was: settingValue(def, 'was' in details ? details.was : undefined, t),
    reset: entry.action === 'settings.reset' || ('reset' in details && details.reset === true),
    locked: 'locked' in details && typeof details.locked === 'boolean' ? details.locked : null,
  };
}

export function auditObjectName(kind: string, t: TFunction): string {
  return t(`everything.kinds.${kind}`, { defaultValue: t('trace.object') });
}
