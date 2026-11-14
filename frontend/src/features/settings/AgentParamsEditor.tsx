/** A place's parameter defaults use the selected model's composer controls and declared option forms. */
import { useQuery } from '@tanstack/react-query';
import { type ReactNode, useCallback, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { agentQuery, providerModelsQuery, providerQuery } from '@/api/resources';
import { targetSettingsQuery, type SettingsTarget } from '@/api/settings';
import { ModelParameterChoice } from '@/features/composer/ModelParameterChoice';
import { statesOf } from '@/features/composer/modelState';
import { nativeMemberSource, nativeParameterViews } from '@/features/models/nativeParameterForm';
import { ParameterInput, parameterInfo } from '@/features/models/ParameterInput';
import { RequestOptions } from '@/features/models/RequestOptions';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { SchemaDraftState } from '@/features/resources/templates/SchemaForm';
import { replaceMember } from '@/features/resources/templates/schema';
import { record } from '@/features/resources/UnitCommands';
import { useDraftFault, useReportValidity } from '@/features/settings/drafts';
import { inheritedTargets } from '@/features/settings/settingParts';
import type { ValueControlProps } from '@/features/settings/ValueControl';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { ModelOption, ParameterView, ResolvedSetting } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { StateLine } from '@/ui/StateLine';
import { ValueSummary } from '@/ui/ValueSummary';

export const AGENT_PARAMS_SETTING = 'agent.params';

export function AgentParamsEditor({ target, setting, scope = null, value, onChange, onEdited, disabled = false }: ValueControlProps & {
  target: SettingsTarget; setting: ResolvedSetting; scope?: string | null;
}) {
  const { t, i18n } = useTranslation();
  const shown = useViewShown();
  const settings = useQuery({ ...targetSettingsQuery(target), staleTime: Infinity });
  const of = (key: string) => settings.data?.find((row) => row.key === key)?.value;
  const providerId = typeof of('agent.default_provider_id') === 'string' ? String(of('agent.default_provider_id')) : '';
  const modelName = typeof of('agent.model') === 'string' ? String(of('agent.model')) : '';
  const provider = useQuery({ ...providerQuery(providerId), enabled: shown && providerId !== '', retry: false });
  const models = useQuery({ ...providerModelsQuery(providerId), enabled: shown && providerId !== '' && modelName !== '', retry: false });
  const chosen = models.data?.find((model) => model.model_name === modelName && model.enabled);
  const profileId = provider.data?.agent?.id ?? (typeof of('agent.default_agent_id') === 'string' ? String(of('agent.default_agent_id')) : '');
  const profile = useQuery({ ...agentQuery(profileId), enabled: shown && profileId !== '', retry: false });
  const own = record(value);
  const above = inheritedTargets(setting, target);
  const features = record(of('agent.features'));
  const locale = i18n.resolvedLanguage ?? 'en';
  const parameters = nativeParameterViews({ descriptor: profile.data?.descriptor, model: chosen?.parameters ?? {}, native: chosen?.option_constraints?.native,
    features, scope, inherited: above, locale, t }).map((parameter) => ({ ...parameter, source: nativeMemberSource(setting, parameter.key) }));
  const option: ModelOption | undefined = chosen === undefined ? undefined : { ...chosen, allowed: true, reason: null };
  const choices = { model: { value: option?.model_name ?? null, options: option === undefined ? [] : [option], source: setting.source, writable: true }, parameters };
  const states = statesOf(choices, own, t);
  const inheritedStates = statesOf(choices, {}, t);
  const controlled = new Set(states.map((state) => state.key));
  const other = parameters.filter((parameter) => !controlled.has(parameter.key));
  const forms = Object.entries(chosen?.parameter_forms ?? {}).filter(([protocol]) => provider.data?.protocols.includes(protocol) === true);
  const known = new Set([...parameters.map((parameter) => parameter.key), ...forms.map(([protocol]) => `provider_options:${protocol}`)]);
  const extra = Object.keys(own).filter((key) => !known.has(key));
  const [query, setQuery] = useState('');
  const needle = query.trim().toLocaleLowerCase();
  const matches = (key: string, title: string) => needle === '' || `${title} ${key}`.toLocaleLowerCase().includes(needle);
  const write = (key: string, next: unknown) => { onEdited?.(); onChange(replaceMember(own, key, next)); };
  const [invalid, setInvalid] = useState<ReadonlySet<string>>(new Set());
  const edit = useRef(onEdited);
  useLayoutEffect(() => { edit.current = onEdited; });
  const report = useCallback((id: string, wrong: boolean) => {
    if (wrong) edit.current?.();
    setInvalid((before) => {
      if (before.has(id) === wrong) return before;
      const next = new Set(before); if (wrong) next.add(id); else next.delete(id); return next;
    });
  }, []);
  useReportValidity(invalid.size > 0 ? t('settings.compact.invalidValue') : null);
  const failure = provider.error ?? models.error ?? profile.error;
  return <SchemaDraftState value={report}><div className="flex min-w-0 flex-col gap-3" data-agent-params-editor>
    {providerId === '' || modelName === '' ? <StateLine compact message={t('settingsEditors.params.chooseModel')} />
      : chosen === undefined && models.isSuccess ? <StateLine compact message={t('settingsEditors.summarizer.modelNotOffered', { model: modelName })} />
      : profileId === '' && forms.length === 0 ? <StateLine compact message={t('settingsEditors.params.chooseAgent')} /> : null}
    {failure !== null && <ErrorState compact error={failure} onRetry={() => { if (provider.isError) void provider.refetch(); if (models.isError) void models.refetch(); if (profile.isError) void profile.refetch(); }} />}
    {parameters.length + forms.length + extra.length > 10 && <Input type="search" hideLabel label={t('settings.editor.search')} placeholder={t('settings.editor.searchPlaceholder')} value={query} onChange={(event) => setQuery(event.target.value)} />}
    <div className="flex max-h-[60vh] min-w-0 flex-col gap-3 overflow-y-auto overscroll-contain">
      {states.filter((state) => matches(state.key, state.title)).map((state) => <NativeField key={state.key} parameter={parameters.find((parameter) => parameter.key === state.key)!} own={Object.hasOwn(own, state.key)} disabled={disabled} onReset={() => write(state.key, undefined)}>
        <ModelParameterChoice state={state} writable={!disabled && state.writable} rate={chosen?.parameters.rate ?? null} onChoose={(next) => {
          const inherited = inheritedStates.find((held) => held.key === state.key)?.force;
          write(state.key, next === inherited ? undefined : next);
        }} />
      </NativeField>)}
      {other.filter((parameter) => matches(parameter.key, parameterInfo(parameter, t).title)).map((parameter) => {
        const info = parameterInfo(parameter, t);
        const def = record(parameter.def);
        const current = Object.hasOwn(own, parameter.key) ? own[parameter.key] : Object.hasOwn(above, parameter.key) ? above[parameter.key] : def.resolved_default;
        const constraint = typeof def.provider_constraint === 'object' && def.provider_constraint !== null ? record(def.provider_constraint) : null;
        return <NativeField key={parameter.key} parameter={parameter} own={Object.hasOwn(own, parameter.key)} disabled={disabled} onReset={() => write(parameter.key, undefined)}>
          {info.type !== 'boolean' && info.type !== 'bool' && <div className="flex items-center gap-1 text-sm font-medium"><span>{info.title}</span>{info.description !== '' && <HelpHint term={info.title} what={info.description} />}</div>}
          <ParameterInput info={info} parameterKey={parameter.key} value={current} constraint={constraint} disabled={disabled} draft onChange={(next) => write(parameter.key, next)} />
        </NativeField>;
      })}
      {forms.length > 0 && <ResourceDetails label={t('providerOptions.title')} keepMounted pauseEffects={false}>
        {forms.filter(([protocol]) => matches(protocol, protocol)).map(([protocol, schema]) => <RequestOptions key={protocol} schema={schema} label={protocol} hideHeading
          value={Object.hasOwn(own, `provider_options:${protocol}`) ? own[`provider_options:${protocol}`] : above[`provider_options:${protocol}`]} disabled={disabled}
          onChange={(next) => write(`provider_options:${protocol}`, Object.keys(record(next)).length === 0 && !Object.hasOwn(above, `provider_options:${protocol}`) ? undefined : next)} />)}
      </ResourceDetails>}
      {extra.length > 0 && <ResourceDetails label={t('settingsEditors.params.other')}>
        <ul className="flex min-w-0 flex-col gap-2">{extra.filter((key) => matches(key, key)).map((key) => <li key={key} className="flex min-w-0 items-start gap-2">
          <div className="min-w-0 flex-1"><span className="text-sm font-medium wrap-anywhere">{key}</span><ValueSummary value={own[key]} /></div>
          <IconButton action="remove" size="sm" label={t('settingsEditors.params.remove', { name: key })} disabled={disabled} onClick={() => write(key, undefined)} />
        </li>)}</ul>
      </ResourceDetails>}
    </div>
  </div></SchemaDraftState>;
}

function NativeField({ parameter, own, disabled, onReset, children }: {
  parameter: ParameterView; own: boolean; disabled: boolean; onReset: () => void; children: ReactNode;
}) {
  const { t } = useTranslation();
  const fault = useDraftFault(parameter.key);
  return <div className="flex min-w-0 flex-col gap-1" data-default-parameter={parameter.key}>
    {children}
    {own && <div><IconButton action="inheritSetting" size="sm" label={t('inspector.parameters.reset', { name: parameterInfo(parameter, t).title })} disabled={disabled} onClick={onReset} /></div>}
    {fault !== undefined && <p role="alert" className="text-sm text-danger-text">{fault}</p>}
  </div>;
}
