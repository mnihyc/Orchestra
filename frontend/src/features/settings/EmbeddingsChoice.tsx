/** The embeddings of search by meaning, chosen in their setting row: the provider by its name among those the reader may use, and the model, length and prefixes as the registry's members.
 *
 * The row's own revision, lock and saving stay the only writer: this hands back the whole value each time.
 */
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import { ProviderDefaultChoice, ProviderDefaultName } from '@/features/settings/ProviderChoice';
import { type Shape, words } from '@/features/settings/settingsModel';
import { searchFieldName } from '@/features/settings/settingText';
import { ValueControl, type ValueControlProps } from '@/features/settings/ValueControl';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { ValueSummary } from '@/ui/ValueSummary';
import { settingChoice } from '@/i18n/choices';

/** The key whose value names the embeddings provider and model. */
export const EMBEDDINGS_SETTING = 'search.semantic.embeddings';
export const SEMANTIC_TUNING_SETTING = 'search.semantic.tuning';
/** Native word retrieval budgets use the same structured setting controls. */
export const WORD_TUNING_SETTING = 'search.word_tuning';

function memberLabel(t: TFunction): (name: string) => string {
  return (name) => searchFieldName(name, t) ?? words(name);
}

/** The normal structured controls with the semantic-index fields named in the reader's language. */
export function SemanticTuning(props: ValueControlProps) {
  const { t } = useTranslation();
  const label = memberLabel(t);
  return <ValueControl {...props} memberLabel={label}
    memberDisplay={(name) => name === 'exact_filter_candidates' ? { unit: 'raw', zero: 'off' } : undefined}
    say={(value) => settingChoice(t, SEMANTIC_TUNING_SETTING, value)?.name ?? props.say?.(value) ?? words(value)} />;
}

/** Tuning values retain their numeric units in the existing compact structured summary. */
export function SemanticTuningSummary({value,inControl=false,label}:{value:unknown;inControl?:boolean;label?:string|undefined}) {
  const {t}=useTranslation();
  const locale=useAppearance((state)=>state.locale);
  const timeZone=useAppearance((state)=>state.timeZone);
  const labelField=memberLabel(t);
  const summary=isRecord(value)?Object.fromEntries(Object.entries(value).map(([name,entry])=>[name,
    name==='exact_filter_candidates'&&entry===0?t('common.off'):
      (name==='interval_secs'||name==='query_wait_secs')&&typeof entry==='number'?formatSpan(entry*1000,{locale,timeZone}):entry])):value;
  return <ValueSummary value={summary} label={label} inControl={inControl} fieldName={labelField} number={(number)=>formatNumber(number,locale)}
    word={(word)=>settingChoice(t,SEMANTIC_TUNING_SETTING,word)?.name??word}/>;
}

/** The member that names the provider; the others are edited as the registry draws them. */
const PROVIDER_MEMBER = 'provider_id';

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

export function EmbeddingsChoice(props: ValueControlProps) {
  const { t } = useTranslation();
  const { shape, value, onChange, disabled } = props;
  const labelField = memberLabel(t);
  if (shape.type !== 'object') return <ValueControl {...props} />;
  const held = isRecord(value) ? value : {};
  const provider = typeof held[PROVIDER_MEMBER] === 'string' ? held[PROVIDER_MEMBER] : '';
  const rest: Shape = { ...shape, members: shape.members.filter((member) => member.name !== PROVIDER_MEMBER) };
  const others = Object.fromEntries(Object.entries(held).filter(([name]) => name !== PROVIDER_MEMBER));
  return (
    <div className="flex flex-col gap-4">
      <ProviderDefaultChoice
        shape={{ type: 'id' }}
        value={provider}
        label={t('admin.semantic.provider')}
        showLabel
        hideLabel
        disabled={disabled === true}
        onChange={(next) => onChange({ ...held, [PROVIDER_MEMBER]: next })}
        {...(props.onEdited === undefined ? {} : { onEdited: props.onEdited })}
      />
      <ValueControl
        {...props}
        shape={rest}
        memberLabel={labelField}
        value={others}
        hideLabel
        onChange={(next) => onChange({ ...(isRecord(next) ? next : {}), [PROVIDER_MEMBER]: provider })}
      />
    </div>
  );
}

/** The embeddings named, as a row's summary reads them: the provider by its name, the model and the length. */
export function EmbeddingsSummary({ value }: { value: unknown }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  if (!isRecord(value) || typeof value[PROVIDER_MEMBER] !== 'string' || value[PROVIDER_MEMBER] === '') return <span className="text-text-muted">{t('admin.semantic.notSet')}</span>;
  const named = [typeof value.model_name === 'string' ? value.model_name : '', typeof value.dimensions === 'number' ? formatNumber(value.dimensions, locale) : '']
    .filter((part) => part !== '')
    .join(' · ');
  return <ProviderDefaultName value={value[PROVIDER_MEMBER]}>{(provider) => (
    <span className="inline-flex min-w-0 flex-wrap gap-x-1">
      <span>{provider.name}</span>
      <span>·</span>
      <span>{named}</span>
    </span>
  )}</ProviderDefaultName>;
}
