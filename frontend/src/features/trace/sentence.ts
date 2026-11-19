/** An audit row already names its actor; the action sentence names only what changed. */
import type { TFunction } from 'i18next';
import { kindOf } from './catalogue';
import { recordedHolder } from './requests';
import { auditActionName, auditSetting } from './words';
import type { AuditEntry as TraceEntry } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { formatNumber } from '@/lib/format';
const DELETIONS = ['node.deleted', 'document.deleted', 'environment.deleted'];
export function auditSentence(entry: TraceEntry, t: TFunction): string {
  if (kindOf(entry.action) === undefined) return entry.summary===entry.action || entry.summary.trim()==='' ? t('trace.unrecognizedAct') : t('trace.unknownAction', { summary: entry.summary });
  const details = entry.details;
  if (entry.action === 'person.notified' && typeof details === 'object' && details !== null && 'count' in details && typeof details.count === 'number') {
    return t('trace.personNotified', { count: details.count, number: formatNumber(details.count, i18n.resolvedLanguage ?? i18n.language) });
  }
  if(entry.action==='provider.window_started_over' && typeof details==='object' && details!==null
    && 'window_label' in details && typeof details.window_label==='string'
    && 'before_percent' in details && typeof details.before_percent==='number'
    && 'after_percent' in details && typeof details.after_percent==='number') {
    const locale=i18n.resolvedLanguage ?? i18n.language;
    return t('trace.windowStartedOver',{window:details.window_label,before:formatNumber(details.before_percent,locale),after:formatNumber(details.after_percent,locale),cause:t('cause' in details && details.cause==='outside' ? 'planUsage.resetCause.outside' : 'planUsage.resetCause.vendor')});
  }
  const setting = auditSetting(entry, t);
  if (setting !== null) return setting.reset
    ? t('trace.sentenceSettingReset', { setting: setting.title })
    : setting.value === null ? t('trace.sentenceSettingChanged', { setting: setting.title })
      : t('trace.sentenceSettingSet', { setting: setting.title, value: setting.value });
  if (entry.action.startsWith('settings.') || entry.action === 'system.settings_updated') return t('trace.sentenceSettingChanged', { setting: t('trace.settings') });
  if (entry.action === 'admin.opened') return entry.object.name === null || entry.object.name.trim() === ''
    ? t('trace.sentenceAdminOpenedUnnamed') : t('trace.sentenceAdminOpened', { name: entry.object.name });
  if (entry.action === 'auth.admin_mode_changed' && typeof details === 'object' && details !== null
    && 'as_admin' in details && typeof details.as_admin === 'boolean') {
    return t(details.as_admin ? 'trace.sentenceAdminOn' : 'trace.sentenceAdminOff');
  }
  if (entry.summary === entry.action || entry.summary.trim() === '') return auditActionName(entry.action, t);
  // Legacy rows concatenate the actor, wire action and target. The actor already has its chip;
  // keep the native target here and translate the known action, leaving the original in Details.
  if (entry.summary.split(' · ').includes(entry.action)) return t('trace.sentenceObjectAction', { action: auditActionName(entry.action, t), name: entry.object.name ?? t('trace.unknown') });
  if (typeof details !== 'object' || details === null || !('version' in details) || details.version !== 1) return entry.summary;
  const holder = recordedHolder(details);
  if (entry.action === 'permission.added' && holder !== null && 'permission' in details && typeof details.permission === 'string') return t('trace.sentencePermissionAdded', { holder });
  if (!('name' in details) || typeof details.name !== 'string') return entry.summary;
  return DELETIONS.includes(entry.action) ? t('trace.sentenceDeleted', { name: details.name }) : entry.summary;
}
