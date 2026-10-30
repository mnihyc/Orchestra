/** Known import reasons use the reader's language; unfamiliar ones keep the server's sentence. */
import type { TFunction } from 'i18next';
import { i18n, type TranslationKey } from '@/i18n';
import { formatBytes, formatNumber } from '@/lib/format';

/** A reason supplied with a plan, report or job. */
export interface ImportReason {
  code: string;
  message: string;
  params?: unknown;
}

/** The finite import vocabulary also makes each authored sentence's product reader explicit. */
function knownReason(code: string, t: TFunction): string | null {
  switch (code) {
    case 'native_sessions': return t('importWizard.reasons.native_sessions');
    case 'workspaces': return t('importWizard.reasons.workspaces');
    case 'secrets': return t('importWizard.reasons.secrets');
    case 'usage_and_audit': return t('importWizard.reasons.usage_and_audit');
    case 'personal_state': return t('importWizard.reasons.personal_state');
    case 'web_images': return t('importWizard.reasons.web_images');
    case 'not_exportable': return t('importWizard.reasons.not_exportable');
    case 'too_large': return t('importWizard.reasons.too_large');
    case 'sharing_not_restored': return t('importWizard.reasons.sharing_not_restored');
    case 'references_outside': return t('importWizard.reasons.references_outside');
    case 'unsupported_block': return t('importWizard.reasons.unsupported_block');
    case 'missing_file': return t('importWizard.reasons.missing_file');
    case 'unknown_person': return t('importWizard.reasons.unknown_person');
    case 'history_dropped': return t('importWizard.reasons.history_dropped');
    case 'org_as_project': return t('importWizard.reasons.org_as_project');
    case 'warnings': return t('importWizard.reasons.warnings');
    case 'import.forbidden': return t('importWizard.reasons.import.forbidden');
    case 'import.disabled': return t('importWizard.reasons.import.disabled');
    case 'import.too_deep': return t('importWizard.reasons.import.too_deep');
    case 'import.unsupported': return t('importWizard.reasons.import.unsupported');
    case 'import.unreadable': return t('importWizard.reasons.import.unreadable');
    case 'import.damaged': return t('importWizard.reasons.import.damaged');
    case 'import.refused': return t('importWizard.reasons.import.refused');
    case 'import.parent_failed': return t('importWizard.reasons.import.parent_failed');
    case 'import.exists': return t('importWizard.reasons.import.exists');
    default: return null;
  }
}

/** Resolve only words actually authored in this language, rather than relying on a fallback catalogue. */
export function importReason(reason: ImportReason, t: TFunction, locale: string): string {
  const params = typeof reason.params === 'object' && reason.params !== null && !Array.isArray(reason.params) ? reason.params as Record<string, unknown> : {};
  const number = (key: string) => { const value = params[key]; return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null; };
  const asked = number('asked'), limit = number('limit');
  if (asked !== null && limit !== null && reason.code === 'import.too_large') return t('importWizard.reasons.import.tooLargeWith', { asked: formatBytes(asked, locale), limit: formatBytes(limit, locale) });
  if (asked !== null && limit !== null && reason.code === 'import.too_many') return t('importWizard.reasons.import.tooManyWith', { asked: formatNumber(asked, locale), limit: formatNumber(limit, locale) });
  const kind = (value: unknown) => typeof value === 'string' && ['org', 'project', 'topic', 'thread', 'folder', 'document', 'chat'].includes(value)
    ? t(`importWizard.reasons.kinds.${value}` as 'importWizard.reasons.kinds.org') : null;
  const child = kind(params.kind), parent = kind(params.parent_kind);
  if (reason.code === 'import.unsupported' && child !== null && parent !== null) return t('importWizard.reasons.import.unsupportedIn', { kind: child, parent });
  if (reason.code === 'sharing_not_restored' && params.reason === 'needs_manage') return t('importWizard.reasons.sharingNeedsManage');
  if (reason.code === 'sharing_not_restored' && params.reason === 'not_carried') return t('importWizard.reasons.sharingNotCarried');
  if (reason.code === 'missing_file' && typeof params.name === 'string') return t('importWizard.reasons.missingFileNamed', { name: params.name });
  const known = knownReason(reason.code, t);
  if (known !== null) return known;
  const key = `errors.${reason.code}`;
  if (typeof i18n.getResource(locale, 'translation', key) !== 'string') return reason.message;
  const sentence: unknown = t(key as TranslationKey, { ...params, returnObjects: false, defaultValue: reason.message });
  return typeof sentence === 'string' ? sentence : reason.message;
}
