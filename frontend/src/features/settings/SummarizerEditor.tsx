/** The summarizer, drawn as a form: which kind, which provider and model, how many tokens, the model's request options and the instructions it is given.
 *
 * A model count left at its built-in value is left out of what is written, so that the saved value says only what the person chose. The raw value stays one press
 * away for a kind this form does not know. The provider and the model are asked of the server as they are chosen, so that a choice that cannot be used says
 * so beside its field before it is saved, not at a later run.
 */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ApiError } from '@/api/client';
import { providerModelsQuery, providerQuery } from '@/api/resources';
import { isRecord, MODEL_SUMMARIZER, NO_SUMMARIZER, type SUMMARIZER_COUNTS, summarizerProblems } from '@/features/settings/draftChecks';
import { useDraftFault, useReportValidity } from '@/features/settings/drafts';
import { ProviderDefaultChoice } from '@/features/settings/ProviderChoice';
import { SummarizerFields } from './SummarizerFields';
import { summarizerCount, summarizerDefaults, summarizerForms, type SummarizerForm } from './SummarizerForms';
import { ValueControl, type ValueControlProps } from '@/features/settings/ValueControl';
import { ModelRequestOptions } from '@/features/models/RequestOptions';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { formatNumber } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Textarea } from '@/ui/Textarea';

/** The built-in answer and context sizes of a model summarizer (`OUTPUT_TOKENS` and `CONTEXT_TOKENS` of the model summarizer), shown where a count is left empty. */
const BUILT_IN_COUNTS = { max_output_tokens: 120, context_tokens: 32_000 } as const;

/** What the kind choice holds for "none here": a choice has no empty value. */
const NONE = '\u0000none';

/** The fields a summary can be written for, each with its own instruction. */
const FIELDS = ['summary', 'title', 'description', 'tags', 'rollup', 'handoff'] as const;

/** The members that only mean something for the provider and model chosen. */
const CHOSEN = ['provider_id', 'model_name', 'params'] as const;

/** The value without the named members. */
function without(held: Record<string, unknown>, names: readonly string[]): Record<string, unknown> {
  return Object.fromEntries(Object.entries(held).filter(([name]) => !names.includes(name)));
}

export function SummarizerEditor({ schema, ...props }: ValueControlProps & { schema?: unknown }) {
  const { t } = useTranslation();
  const { value, onChange, onEdited, disabled = false } = props;
  const held = isRecord(value) ? value : { ...NO_SUMMARIZER };
  const kind = typeof held.kind === 'string' ? held.kind : '';
  const forms = summarizerForms(schema);
  const chosen = forms.find((form) => form.name === kind);
  const known = kind === '' || kind === MODEL_SUMMARIZER || chosen !== undefined;
  const [raw, setRaw] = useState(false);
  const asJson = !known || raw && kind !== '';
  const set = (next: Record<string, unknown>) => { onEdited?.(); onChange(next); };
  const kinds = [
    { value: NONE, label: t('settingsEditors.summarizer.none') },
    { value: MODEL_SUMMARIZER, label: t('settingsEditors.summarizer.model') },
    ...forms.filter((form) => form.name !== MODEL_SUMMARIZER).map((form) => ({ value: form.name, label: form.title })),
    ...(known ? [] : [{ value: kind, label: kind }]),
  ];
  return <div className="flex min-w-0 flex-col gap-3" data-summarizer-editor>
    <ChoiceLocator size="sm" label={t('settingsEditors.summarizer.kind')} value={kind === '' ? NONE : kind} options={kinds} disabled={disabled} fieldClassName="max-w-[20rem]"
      onValueChange={(next) => {
        setRaw(false);
        set(next === NONE ? { ...NO_SUMMARIZER } : next === MODEL_SUMMARIZER ? { kind: MODEL_SUMMARIZER }
          : { ...summarizerDefaults(forms.find((form) => form.name === next)), kind: next });
      }} />
    {kind === '' && <p className="text-sm text-text-muted">{t('settingsEditors.summarizer.noneHere')}</p>}
    {asJson ? <ValueControl {...props} shape={{ type: 'json' }} /> : kind === MODEL_SUMMARIZER ? <ModelForm held={held} set={set} disabled={disabled} form={chosen} />
      : chosen !== undefined && <SummarizerFields {...props} form={chosen} onChange={(next) => { if (isRecord(next)) set(next); }} />}
    {known && kind !== '' && <div><Button size="sm" variant="ghost" disabled={disabled} onClick={() => setRaw((was) => !was)}>{t(asJson ? 'settingsEditors.summarizer.useForm' : 'settingsEditors.summarizer.editJson')}</Button></div>}
  </div>;
}

/** The members of a model summarizer, as fields. */
function ModelForm({ held, set, disabled, form }: { held: Record<string, unknown>; set: (next: Record<string, unknown>) => void; disabled: boolean; form: SummarizerForm | undefined }) {
  const { t } = useTranslation();
  const providerId = typeof held.provider_id === 'string' ? held.provider_id : '';
  const modelName = typeof held.model_name === 'string' ? held.model_name : '';
  const params = isRecord(held.params) ? held.params : {};
  const prompts = isRecord(held.prompts) ? held.prompts : {};
  const fields = form?.fields ?? FIELDS;
  const problems = summarizerProblems(held);
  const provider = useQuery({ ...providerQuery(providerId), enabled: providerId !== '', retry: false });
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: providerId !== '', retry: false });
  const [typed, setTyped] = useState('');
  const refused = (error: unknown) => error instanceof ApiError && [401, 403, 404].includes(error.status);
  const unusable = providerId !== '' && (refused(provider.error) || refused(models.error)
    || provider.data !== undefined && (!holds(provider.data.access, 'provider.use') || provider.data.status === 'disabled'));
  const enabled = (models.data ?? []).filter((model) => model.enabled);
  const serverProvider = useDraftFault('provider_id');
  const serverModel = useDraftFault('model_name');
  const providerFault = problems.includes('provider_id') ? t('settingsEditors.summarizer.chooseProvider') : unusable ? t('settingsEditors.summarizer.cannotUse') : serverProvider;
  const modelFault = problems.includes('model_name') ? t('settingsEditors.summarizer.chooseModel')
    : providerId !== '' && !unusable && models.data !== undefined && !enabled.some((model) => model.model_name === modelName)
      ? t(enabled.length === 0 ? 'settingsEditors.summarizer.noModels' : 'settingsEditors.summarizer.modelNotOffered', { model: modelName }) : serverModel;
  useReportValidity(providerFault ?? modelFault ?? null);
  const match = typed.trim().toLocaleLowerCase();
  const options = enabled.filter((model) => match === '' || `${model.display_name} ${model.model_name}`.toLocaleLowerCase().includes(match))
    .map((model) => ({ value: model.model_name, label: model.display_name.trim() === '' ? model.model_name : model.display_name, text: model.display_name.trim() === '' ? model.model_name : model.display_name }));
  return <div className="flex min-w-0 flex-col gap-3">
    <ProviderDefaultChoice shape={{ type: 'id' }} value={providerId} label={t('settingsEditors.summarizer.provider')} showLabel providerKind="api" disabled={disabled}
      error={providerFault} onChange={(next) => set({ ...without(held, CHOSEN), provider_id: next, model_name: '' })} />
    <Combobox label={t('settingsEditors.summarizer.modelName')} query={typed} onQueryChange={setTyped} displayValue={modelName} disabled={disabled || providerId === ''}
      placeholder={providerId === '' ? t('settingsEditors.summarizer.providerFirst') : t('settingsEditors.summarizer.chooseModel')} loading={models.isFetching} error={modelFault}
      groups={[{ id: 'models', label: '', options }]} empty={t('ui.combobox.empty')} onOpenChange={(open) => { if (!open) setTyped(''); }}
      onSelect={(option) => { setTyped(''); set({ ...without(held, ['params']), model_name: option.value }); }} />
    <div className="grid min-w-0 grid-cols-1 gap-3 @min-[28rem]:grid-cols-2">
      <CountField member="max_output_tokens" label={t('settingsEditors.summarizer.answerTokens')} held={held} set={set} disabled={disabled} wrong={problems.includes('max_output_tokens')} builtIn={summarizerCount(form, 'max_output_tokens')} />
      <CountField member="context_tokens" label={t('settingsEditors.summarizer.contextTokens')} held={held} set={set} disabled={disabled} wrong={problems.includes('context_tokens')} builtIn={summarizerCount(form, 'context_tokens')} />
    </div>
    <ResourceDetails label={t('settingsEditors.summarizer.requestOptions')}>
      {() => <ModelRequestOptions providerId={providerId} model={modelName} hideHeading disabled={disabled} value={params}
        onChange={(next) => set(isRecord(next) && Object.keys(next).length > 0 ? { ...held, params: next } : without(held, ['params']))} />}
    </ResourceDetails>
    {fields.length > 0 && <ResourceDetails label={t('settingsEditors.summarizer.instructions')}>
      {() => <div className="flex min-w-0 flex-col gap-3">
        {fields.map((field) => <Textarea key={field} rows={2} label={t(`settingsEditors.summarizer.fields.${field}`, { defaultValue: field })} disabled={disabled}
          placeholder={t('settingsEditors.summarizer.builtInText')} value={textOf(prompts[field])}
          onChange={(event) => {
            const next = Object.fromEntries(Object.entries(prompts).filter(([name]) => name !== field));
            if (event.target.value.trim() !== '') next[field] = event.target.value;
            set(Object.keys(next).length === 0 ? without(held, ['prompts']) : { ...held, prompts: next });
          }} />)}
      </div>}
    </ResourceDetails>}
    {(models.error !== null && !refused(models.error)) && <ErrorAlert error={models.error} />}
  </div>;
}

const textOf = (value: unknown): string => typeof value === 'string' ? value : '';

/** One of the two counts of a model summarizer: empty keeps the built-in count, which it shows as its placeholder. */
function CountField({ member, label, held, set, disabled, wrong, builtIn }: { member: (typeof SUMMARIZER_COUNTS)[number]; label: string; held: Record<string, unknown>; set: (next: Record<string, unknown>) => void; disabled: boolean; wrong: boolean; builtIn: number | undefined }) {
  const { t, i18n } = useTranslation();
  const given = held[member];
  const fault = useDraftFault(member);
  return <Input type="number" inputMode="numeric" min={1} step={1} label={label} disabled={disabled} fieldClassName="min-w-0"
    value={typeof given === 'number' ? given : ''} placeholder={formatNumber(builtIn ?? BUILT_IN_COUNTS[member], i18n.resolvedLanguage ?? i18n.language)}
    error={wrong ? t('settingsEditors.summarizer.atLeastOne') : fault}
    onChange={(event) => {
      const text = event.target.value.trim();
      if (text === '') set(without(held, [member]));
      else if (/^-?\d+$/u.test(text)) set({ ...held, [member]: Number(text) });
    }} />;
}
