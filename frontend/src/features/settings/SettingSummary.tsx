/** A setting's actual values keep their units and familiar choice names in compact summaries. */
import { useTranslation } from 'react-i18next';
import { useMemo } from 'react';
import type { TFunction } from 'i18next';
import { settingChoice } from '@/i18n/choices';
import type { SettingDef } from '@/generated/api-types';
import { numericSettingDisplay, numericSettingText, settingFieldName } from '@/features/settings/settingText';
import { formatNumber } from '@/lib/format';
import { ValueSummary } from '@/ui/ValueSummary';
import { modelDisplayName } from '@/features/models/names';
import { ProviderDefaultName } from '@/features/settings/ProviderChoice';
import { EMBEDDINGS_SETTING, EmbeddingsSummary, SEMANTIC_TUNING_SETTING, WORD_TUNING_SETTING, SemanticTuningSummary } from '@/features/settings/EmbeddingsChoice';
import { isResourceSetting, ResourceSettingName } from '@/features/settings/ResourceChoice';
import { SKILLS_SETTING, SkillsSummary } from '@/features/settings/SkillsChoice';
import { PathValue } from '@/features/resources/TechnicalValue';
import { PROGRAM_VERSIONS_KEY, versionChoice } from './versionPolicy';
import { choiceCopy } from '@/i18n/choices';
import { CallsSummary } from './CallsRules';
import { useHarnessNames } from '@/features/resources/harnesses';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import { ErrorState } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';

export function emptySettingValue(value: unknown): boolean {
  return value === null || value === undefined || value === '' || (typeof value === 'object' && Object.keys(value).length === 0);
}

/** A missing semantic choice and an unlimited disk share use their product meanings. */
export function emptySettingText(key: string | undefined, defaultValue: boolean, t: TFunction): string {
  if (key === EMBEDDINGS_SETTING) return t('admin.semantic.notSet');
  if (key === 'search.semantic.max_disk_share') return t('admin.semantic.noLimit');
  return t(defaultValue ? 'settings.compact.defaultNone' : 'settings.compact.none');
}

function ProgramPolicySummary({ value }: { value: object }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const keys = useMemo(() => Object.keys(value), [value]);
  const { kinds, title } = useHarnessNames(keys.length > 0);
  if (keys.length === 0) return <>{choiceCopy(t, 'harnessVersion.shipped').name}</>;
  if (kinds.isPending) return <StateLine compact message={t('programList.loadingNames')} />;
  return <div className="flex min-w-0 flex-col gap-2">
    {kinds.isError && <ErrorState compact error={kinds.error} retrying={kinds.isRefetching} onRetry={() => void kinds.refetch()} />}
    <CurrentDetailPage count={keys.length}>{(from, to) => {
      const choices = keys.slice(from, to).map((kind) => {
        const choice = versionChoice(value, kind);
        const label = choiceCopy(t, `harnessVersion.${choice.use}`).name;
        return t('programVersions.policySummary', { name: title(kind), choice: choice.use === 'exact' && choice.version !== undefined ? t('programVersions.exactSummary', { choice: label, version: choice.version }) : label });
      });
      return <span className="min-w-0 wrap-anywhere">{new Intl.ListFormat(locale, { style: 'long', type: 'conjunction' }).format(choices)}</span>;
    }}</CurrentDetailPage>
  </div>;
}

export function SettingSummary({ value, def, inControl = false, example = false }: { value: unknown; def: Partial<Pick<SettingDef, 'key' | 'title' | 'display' | 'schema'>>; inControl?: boolean; example?: boolean }) {
  const { t, i18n } = useTranslation();
  const locale=i18n.resolvedLanguage??'en';
  const display=numericSettingDisplay(def);
  const number=(value:number)=>display===null ? formatNumber(value,locale) : numericSettingText(value,display,locale,t);
  if (def.key === PROGRAM_VERSIONS_KEY && typeof value === 'object' && value !== null && !Array.isArray(value)) {
    return <ProgramPolicySummary value={value} />;
  }
  if (def.key === 'composer.paste_inline_context_share' && (value === 0 || value === 100)) return <span>{t(value === 0 ? 'uploads.alwaysFile' : 'uploads.alwaysInline')}</span>;
  if (def.key === 'agent.default_provider_id') return example ? <span>{t('places.providerDefault.example')}</span> : <ProviderDefaultName value={value} />;
  if (def.key === EMBEDDINGS_SETTING && !example) return <EmbeddingsSummary value={value} />;
  if (def.key === 'search.semantic.max_disk_share' && emptySettingValue(value)) return <span>{emptySettingText(def.key, false, t)}</span>;
  // A goal's credits are a decimal, and zero is no limit.
  if (def.key === 'thread.goal.max_credits' && (value === '0' || value === 0)) return <span>{t('goals.editor.noLimit')}</span>;
  if (def.key === SEMANTIC_TUNING_SETTING || def.key === WORD_TUNING_SETTING) return <SemanticTuningSummary value={value} label={def.title} inControl={inControl} />;
  if (def.key === 'auto.summarizer' && !example && typeof value === 'object' && value !== null && 'kind' in value && value.kind === 'model' && 'provider_id' in value) {
    return <ProviderDefaultName value={value.provider_id}>{(provider) => <ValueSummary
      value={{ ...value, provider_id: provider.name }} label={def.title} inControl={inControl}
      fieldName={(name) => settingFieldName(name, def.key, t)} number={number} />}</ProviderDefaultName>;
  }
  if (def.key !== undefined && isResourceSetting(def.key)) return <ResourceSettingName settingKey={def.key} value={value} example={example} />;
  if (def.key === 'runtime.home' && typeof value === 'object' && value !== null && 'kind' in value) {
    if (value.kind === 'existing_directory' && 'path' in value && typeof value.path === 'string') return <PathValue path={value.path} />;
    if (value.kind === 'made_by_orch') return <span>{t('environmentData.made')}</span>;
  }
  if (def.key === SKILLS_SETTING) return <SkillsSummary value={value} />;
  if (def.key === 'calls.rules') return <CallsSummary value={value} />;
  if(def.key==='mcp.enabled' && typeof value==='object' && value!==null && !Array.isArray(value)){
    const flags:unknown[]=Object.values(value);
    if(flags.every((flag)=>typeof flag==='boolean')){
      const enabled=flags.filter((flag)=>flag).length;
      const disabled=flags.length-enabled;
      return <span className="inline-flex flex-wrap gap-x-3 gap-y-1">{flags.length===0?t('screenReview.emptyObject'):<>{enabled>0 && <span>{t('settings.compact.enabledCount',{count:enabled})}</span>}{disabled>0 && <span>{t('settings.compact.disabledCount',{count:disabled})}</span>}</>}</span>;
    }
  }
  const isModel = def.key === 'agent.model';
  return <ValueSummary value={value} label={def.title} inControl={inControl}
    word={(text) => isModel ? modelDisplayName(text, t) : settingChoice(t, def.key ?? null, text)?.name ?? text}
    fieldName={(name) => settingFieldName(name, def.key, t)}
    number={number} />;
}
