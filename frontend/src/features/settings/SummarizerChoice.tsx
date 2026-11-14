/** Registered producers supply a kind choice and a form; the settings row keeps its own draft and save. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { providerModelsQuery } from '@/api/resources';
import { record } from '@/features/resources/UnitCommands';
import { ModelRequestOptions } from '@/features/models/RequestOptions';
import { modelDisplayName } from '@/features/models/names';
import { ProviderDefaultChoice } from './ProviderChoice';
import { shapeOf, words } from './settingsModel';
import { ValueControl, type ValueControlProps } from './ValueControl';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';
import { Checkbox } from '@/ui/Checkbox';
import { Textarea } from '@/ui/Textarea';

interface Producer { name: string; title: string; config_schema: unknown; fields: string[] }

function producers(schema: unknown): Producer[] {
  const offered = record(schema)['x-summarizers'];
  return Array.isArray(offered) ? offered.flatMap((item) => {
    const row = record(item);
    return typeof row.name === 'string' && row.name !== '' && typeof row.title === 'string'
      ? [{ name: row.name, title: row.title, config_schema: row.config_schema, fields: Array.isArray(row.fields) ? row.fields.filter((field): field is string => typeof field === 'string') : [] }] : [];
  }) : [];
}

function defaults(schema: unknown): Record<string, unknown> {
  return Object.fromEntries(Object.entries(record(record(schema).properties)).flatMap(([name, field]) => {
    const value = record(field);
    return Object.hasOwn(value, 'default') ? [[name, value.default]] : [];
  }));
}

export function SummarizerChoice({ schema, ...props }: ValueControlProps & { schema?: unknown }) {
  const { t } = useTranslation();
  const held = record(props.value);
  const kinds = producers(schema);
  const kind = typeof held.kind === 'string' ? held.kind : '';
  const chosen = kinds.find((candidate) => candidate.name === kind);
  const change = (value: unknown) => { props.onEdited?.(); props.onChange(value); };
  const configuration = Object.fromEntries(Object.entries(held).filter(([name]) => name !== 'kind'));
  return <div className="flex min-w-0 flex-col gap-2">
    <ChoiceLocator label={props.label} hideLabel={props.hideLabel === true} value={kind} disabled={props.disabled === true}
      placeholder={t('settings.summarizer.choose')} options={kinds.map((producer) => ({ value: producer.name, name: producer.name === 'model' ? t('settings.summarizer.modelKind') : producer.title }))}
      valueLabel={chosen === undefined && kind !== '' ? t('settings.summarizer.unavailable') : undefined}
      onValueChange={(next) => { const producer = kinds.find((item) => item.name === next); if (producer !== undefined) change({ kind: next, ...defaults(producer.config_schema) }); }} />
    {chosen === undefined ? kind === '' && kinds.length > 0 ? null : <StateLine title={t(kind === '' ? 'settings.summarizer.none' : 'settings.summarizer.unavailable')} />
      : chosen.name === 'model' ? <ModelConfiguration {...props} value={held} producer={chosen} onChange={change} />
      : <ValueControl {...props} shape={shapeOf(chosen.config_schema)} value={configuration} hideLabel
        memberLabel={(name) => { const field = record(record(record(chosen.config_schema).properties)[name]); return typeof field.title === 'string' ? field.title : words(name); }}
        onChange={(next) => change({ kind, ...record(next) })} />}
  </div>;
}

function ModelConfiguration({ producer, ...props }: ValueControlProps & { producer: Producer }) {
  const { t } = useTranslation();
  const held = record(props.value), provider = typeof held.provider_id === 'string' ? held.provider_id : '', model = typeof held.model_name === 'string' ? held.model_name : '';
  const properties = record(record(producer.config_schema).properties);
  const otherSchema = { ...record(producer.config_schema), properties: Object.fromEntries(Object.entries(properties).filter(([name]) => !['provider_id', 'model_name', 'params', 'prompts'].includes(name))), additionalProperties: false };
  const remaining = Object.fromEntries(Object.entries(held).filter(([name]) => !['kind', 'provider_id', 'model_name', 'params', 'prompts'].includes(name)));
  const prompts = record(held.prompts);
  const labels: Readonly<Record<string, string>> = {
    context_tokens: t('settings.summarizer.fields.context_tokens'), max_output_tokens: t('settings.summarizer.fields.max_output_tokens'),
    title: t('settings.summarizer.fields.title'), description: t('settings.summarizer.fields.description'), tags: t('settings.summarizer.fields.tags'),
    rollup: t('settings.summarizer.fields.rollup'), handoff: t('settings.summarizer.fields.handoff'), summary: t('settings.summarizer.fields.summary'),
  };
  const fieldName = (name: string) => labels[name] ?? words(name);
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="grid min-w-0 grid-cols-1 gap-2 sm:grid-cols-2">
      <ProviderDefaultChoice {...props} shape={{ type: 'id' }} value={provider} label={t('settings.summarizer.provider')} showLabel
        onChange={(next) => props.onChange({ ...held, provider_id: next, model_name: '' })} />
      <ModelName provider={provider} value={model} disabled={props.disabled === true} onChange={(next) => props.onChange({ ...held, model_name: next })} />
    </div>
    <details className="min-w-0" data-summarizer-options><summary className="focus-inset cursor-default rounded-md py-1 text-sm text-text-muted">{t('settings.summarizer.options')}</summary>
      <div className="flex min-w-0 flex-col gap-3 py-2">
        <ValueControl {...props} shape={shapeOf(otherSchema)} value={remaining} hideLabel memberLabel={fieldName}
          onChange={(next) => props.onChange({ ...Object.fromEntries(Object.entries(held).filter(([name]) => ['kind', 'provider_id', 'model_name', 'params', 'prompts'].includes(name))), ...record(next) })} />
        <ModelRequestOptions providerId={provider} model={model} value={held.params ?? {}} disabled={props.disabled === true}
          onChange={(next) => props.onChange({ ...held, params: next })} />
        {producer.fields.map((field) => <div key={field} className="flex flex-col gap-1">
          <Checkbox label={t('settings.summarizer.instructions', { field: fieldName(field) })} checked={typeof prompts[field] === 'string'} disabled={props.disabled === true}
            onCheckedChange={(on) => props.onChange({ ...held, prompts: { ...prompts, [field]: on ? '' : null } })} />
          {typeof prompts[field] === 'string' && <Textarea label={t('settings.summarizer.instructions', { field: fieldName(field) })} hideLabel rows={3}
            value={prompts[field]} disabled={props.disabled === true} onChange={(event) => props.onChange({ ...held, prompts: { ...prompts, [field]: event.target.value } })} />}
        </div>)}
      </div>
    </details>
  </div>;
}

function ModelName({ provider, value, disabled, onChange }: { provider: string; value: string; disabled: boolean; onChange: (value: string) => void }) {
  const { t } = useTranslation();
  const models = useQuery({ ...providerModelsQuery(provider), enabled: provider !== '', retry: false });
  const enabled = models.data?.filter((model) => model.enabled) ?? [];
  return <div className="flex min-w-0 flex-col gap-1">
    <ChoiceLocator label={t('settings.summarizer.model')} value={value} disabled={disabled || provider === '' || models.isLoading}
      placeholder={t(provider === '' ? 'settings.summarizer.providerFirst' : 'settings.summarizer.chooseModel')}
      valueLabel={value !== '' && models.isPending ? t('common.loading') : value !== '' && !enabled.some((model) => model.model_name === value) ? t('settings.summarizer.modelUnavailable') : undefined}
      options={enabled.map((model) => ({ value: model.model_name, name: model.display_name.trim() ? modelDisplayName(model.display_name, t) : model.model_name }))} onValueChange={onChange} />
    {models.error !== null && <ErrorAlert error={models.error} />}
  </div>;
}
