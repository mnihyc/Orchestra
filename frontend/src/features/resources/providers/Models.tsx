/** A provider's models: the name that is sent and the name that is shown, suggestions from the endpoint's list, and for each the speeds and access programs it offers, enabled one by one.
 *
 * A table of the allowlist with the two names a model has in its first two columns: the display name,
 * which people see and choose and which is changed where it stands, and the routing name, the exact name
 * sent to the endpoint, which is fixed once the model is added. Then the groups the model is in, whether it
 * may be chosen, what it can be run with here at a glance (its efforts, its context, the speeds and access
 * programs that are switched on, and what is offered and not yet on) and where it came from. Above the list
 * stand the provider's groups with how many models each has. A model's row editor (`ModelEditor.tsx`) is
 * where a manager turns a speed, an access program or a larger context on. Suggestions are what the
 * endpoint's own list names that is not listed yet, taken one by one or all at once; what they offer is
 * kept as offered and nothing is switched on by taking them.
 */
import { useMutation } from '@tanstack/react-query';
import type { TFunction } from 'i18next';
import { ArrowRightLeft, Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { acceptSuggestions, refreshProvider, updateModel } from '@/api/resources';
import { formatContext } from '@/features/models/context';
import { AddModelDialog, ModelEditorDialog } from '@/features/resources/providers/ModelEditor';
import { ModelLifecycle } from '@/features/resources/providers/ModelLifecycle';
import { useResourceEnable } from '@/features/storage/Enable';
import { openChange } from '@/features/change/target';
import { GroupChip, GroupStrip } from '@/features/resources/providers/Groups';
import { modelUnavailableAt } from '@/features/resources/providers/plan';
import { detailState, modelsSource } from '@/features/resources/providers/details';
import { SourceLine } from '@/features/resources/providers/ProviderUsageCard';
import { programNameIn } from '@/features/resources/programNames';
import type { ModelParameters, ModelSuggestion, Provider, ProviderModel } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { ActionIcon } from '@/ui/actions';
import { Checkbox } from '@/ui/Checkbox';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';
import { Switch } from '@/ui/Switch';
import { type Column, Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { modelDisplayName } from '@/features/models/names';

const shownName = (model: Pick<ProviderModel, 'display_name' | 'model_name'>,t:TFunction): string => modelDisplayName(model.display_name,t);

/** Source support is separate from an empty discovery and from the manager's enabled model list. */
export function ModelsSourceState({ provider,reading=false }: { provider:Provider;reading?:boolean }) {
  const { t, i18n }=useTranslation();
  const state=detailState(provider,'models');
  const error=modelsSource(provider).last_error;
  const harness = provider.harnesses.find((harness) => harness.machine.id === provider.machine?.id && harness.harness_version !== null);
  const buildReason = state === 'unsupported' && provider.harness_kind !== null && harness !== undefined
    ? t('programVersions.featureOff', { feature: t('programVersions.features.model_discovery'), name: programNameIn(provider, i18n.resolvedLanguage ?? i18n.language) ?? programNameIn(provider.agent, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown'), version: harness.harness_version }) : null;
  return <div className="flex flex-col gap-2" data-models-state={state}>
    <p role={reading && state === 'not_read_yet' ? 'status' : undefined} aria-busy={reading} className="text-sm text-text-muted">{buildReason ?? t(state === 'unsupported' ? 'providerDetails.models.unsupported' : state === 'not_read_yet' ? 'providerDetails.models.notRead' : state === 'empty' ? 'providerDetails.models.empty' : 'providerDetails.models.told')}</p>
    {(state === 'told' || state === 'empty') && <SourceLine provider={provider} field="models" />}
    {error !== null && <ErrorAlert error={error} />}
  </div>;
}

/** Discovery reports an absence; the manager's existing enabled choice remains theirs. */
export function ModelAvailability({ provider, name }: { provider: Provider; name: string }) {
  const { t } = useTranslation();
  const at = modelUnavailableAt(provider, name);
  return at === null ? null : <p className="text-sm text-warning">{t('providerPlan.notOffered')} <RelativeTime at={at} /></p>;
}

/**
 * What a model offers that no manager has switched on yet, in words: the speeds, the access programs and a
 * larger context. A suggestion lists them as what it can do (the source's own list), and a model that is
 * listed keeps them as offered beside what its managers switched on.
 */
export function offeredOnly(parameters: ModelParameters, locale: string, t: TFunction, suggestion = false): string[] {
  const on = new Set(suggestion ? [] : (parameters.service_tiers ?? []).map((tier) => tier.id));
  const tiers = [...(parameters.offered?.service_tiers ?? []), ...(suggestion ? (parameters.service_tiers ?? []) : [])];
  const speeds = [...new Map(tiers.filter((tier) => !on.has(tier.id)).map((tier) => [tier.id, tier.label])).values()];
  const switched = suggestion ? [] : (parameters.access_programs ?? []);
  const programs = [...new Set([...(parameters.offered?.access_programs ?? []), ...(suggestion ? (parameters.access_programs ?? []) : [])])].filter((program) => !switched.includes(program));
  const largest = suggestion ? (parameters.offered?.context_max ?? parameters.context_max) : (parameters.context_max === undefined || parameters.context_max === null ? parameters.offered?.context_max : null);
  const bigger = largest !== undefined && largest !== null ? [t('models.list.contextUpTo', { largest: formatContext(largest, locale) })] : [];
  return [...speeds, ...programs, ...bigger];
}

/** What a model can be run with here, a line for each thing: efforts, context, images, the speeds and programs that are on and what is offered and not yet on, and the rate. */
export function Summary({ parameters }: { parameters: ModelParameters }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const lines: string[] = [];
  if ((parameters.efforts ?? []).length > 0) lines.push(t('providers.models.efforts', { list: (parameters.efforts ?? []).join(', ') }));
  const window = parameters.context_window;
  const largest = parameters.context_max;
  if (window !== null && window !== undefined && largest !== null && largest !== undefined) lines.push(t('models.list.contextBoth', { window: formatContext(window, locale), largest: formatContext(largest, locale) }));
  else if (largest !== null && largest !== undefined) lines.push(t('models.list.contextLargest', { largest: formatContext(largest, locale) }));
  else if (window !== null && window !== undefined) lines.push(t('providers.models.context', { size: formatContext(window, locale) }));
  if (parameters.input_images === true) lines.push(t('providers.models.images'));
  if ((parameters.service_tiers ?? []).length > 0) lines.push(t('providers.models.speeds', { list: (parameters.service_tiers ?? []).map((tier) => tier.label).join(', ') }));
  if ((parameters.access_programs ?? []).length > 0) lines.push(t('providers.models.programs', { list: (parameters.access_programs ?? []).join(', ') }));
  if (parameters.rate !== null && parameters.rate !== undefined) {
    lines.push(t('providers.models.rate', { input: parameters.rate.input_per_mtok, cached: parameters.rate.cached_input_per_mtok, output: parameters.rate.output_per_mtok }));
  }
  const offered = offeredOnly(parameters, locale, t);
  if (lines.length === 0 && offered.length === 0) return <span className="text-text-subtle">{t('providers.models.nothingSaid')}</span>;
  return (
    <ul className="flex flex-col gap-1 text-sm wrap-anywhere">
      {lines.map((line) => (
        <li key={line} className="text-text-muted">
          {line}
        </li>
      ))}
      {offered.length > 0 && <li className="text-text-subtle">{t('models.list.offered', { list: offered.join(', ') })}</li>}
    </ul>
  );
}

// ---------------------------------------------------------------- the table

/** The name people see, changed where it stands by whoever manages the provider. */
function DisplayName({ provider, model, manage }: { provider: Provider; model: ProviderModel; manage: boolean }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(model.display_name);
  const save = useMutation({
    mutationFn: () => updateModel(provider.id, model.id, { display_name: text.trim() }),
    onSuccess: () => {
      setEditing(false);
    },
  });
  const shown = shownName(model,t);
  if (!manage) return <span className="font-medium wrap-anywhere text-text">{shown}</span>;
  if (!editing) {
    return (
      <button
        type="button"
        aria-label={t('providers.models.editName', { name: shown })}
        title={t('providers.models.editName', { name: shown })}
        className="focus-inset group inline-flex max-w-full items-center gap-2 rounded-md text-left font-medium text-text hover:underline"
        onClick={() => {
          setText(model.display_name);
          setEditing(true);
        }}
      >
        <span className="min-w-0 wrap-anywhere">{shown}</span>
        <ActionIcon action="edit" className="text-text-subtle" />
      </button>
    );
  }
  return (
    <form
      className="flex flex-col gap-1"
      onSubmit={(event) => {
        event.preventDefault();
        save.mutate();
      }}
    >
      <div className="flex flex-wrap items-center gap-2">
        <Input
          label={t('providers.models.displayName')}
          hideLabel
          value={text}
          placeholder={model.model_name}
          autoFocus
          disabled={save.isPending}
          fieldClassName="min-w-0 flex-1 basis-[10rem]"
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Escape') setEditing(false);
          }}
          autoComplete="off"
        />
        <Button admin={adminSpec(provider, 'provider.manage')} size="sm" variant="primary" type="submit" pending={save.isPending}>
          {t('providers.models.save')}
        </Button>
        <Button size="sm" disabled={save.isPending} onClick={() => setEditing(false)}>
          {t('common.cancel')}
        </Button>
      </div>
      {save.error !== null && <ErrorAlert error={save.error} />}
    </form>
  );
}

function EnabledSwitch({ provider, model, manage }: { provider: Provider; model: ProviderModel; manage: boolean }) {
  const { t } = useTranslation();
  const [pausing,setPausing]=useState(false);
  const enable = useResourceEnable();
  const name = shownName(model,t);
  if (!manage) return <Badge tone={model.enabled ? 'accent' : 'neutral'}>{model.enabled ? t('providers.models.on') : t('providers.models.off')}</Badge>;
  return (
    <span className="inline-flex flex-col gap-1">
      <Switch admin={adminSpec(provider, 'provider.manage')} label={<span className="sr-only">{t('providers.models.enabledLabel', { name })}</span>} checked={model.enabled} onCheckedChange={(next) => {if(next)enable.ask({target:{object:{kind:'provider',id:provider.id},model_id:model.id},name,admin:adminSpec(provider,'provider.manage'),enable:(runWaiting)=>updateModel(provider.id,model.id,{enabled:true},undefined,runWaiting)});else setPausing(true);}} />
      {enable.dialogs}
      {pausing && <ModelLifecycle provider={provider} model={model} kind="disable" onClose={()=>setPausing(false)} />}
    </span>
  );
}

function Suggestions({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const state = detailState(provider,'models');
  const refresh=useMutation({mutationFn:() => refreshProvider(provider.id)});
  const [chosen, setChosen] = useState<readonly string[]>([]);
  const accept = useMutation({
    mutationFn: (names: string[]) => acceptSuggestions(provider.id, names),
    onSuccess: (added) => {
      setChosen([]);
      toast({ tone: 'success', title: t('providers.models.suggestionsAdded', { count: added.length }) });
    },
  });
  const listed = new Set(provider.models.map((model) => model.model_name));
  const suggestions: ModelSuggestion[] = (provider.facts.models ?? []).filter((suggestion) => !listed.has(suggestion.model_name));

  return (
    <Section id="provider-suggestions" title={t('providers.models.suggestions')} lead={t('providers.models.suggestionsNote')}>
      <ModelsSourceState provider={provider} reading={refresh.isPending} />
      {state !== 'unsupported' && <Button admin={adminSpec(provider, 'provider.manage')} size="sm" className="self-start" pending={refresh.isPending} onClick={() => refresh.mutate()}>{t('providers.overview.refresh')}</Button>}
      {refresh.error !== null && <ErrorAlert error={refresh.error} />}
      {state !== 'told' ? null : suggestions.length === 0 ? (
        <EmptyState compact title={t('providers.models.noSuggestions')} />
      ) : (
        <div className="flex flex-col gap-3">
          <ul aria-label={t('providers.models.suggestions')} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
            {suggestions.map((suggestion) => {
              const rate = suggestion.parameters.rate;
              const offered = offeredOnly(suggestion.parameters, locale, t, true);
              return (
                <li key={suggestion.model_name} className="px-3 py-2">
                  <Checkbox
                    label={
                      <span className="flex flex-wrap items-baseline gap-x-2">
                        <span className="font-medium">{modelDisplayName(suggestion.display_name,t)}</span>
                        {suggestion.display_name !== suggestion.model_name && <span className="text-sm text-text-muted">{t('providers.models.routingName')}: <code>{suggestion.model_name}</code></span>}
                        {rate !== undefined && rate !== null && (
                          <span className="text-sm text-text-muted">{t('providers.models.rate', { input: rate.input_per_mtok, cached: rate.cached_input_per_mtok, output: rate.output_per_mtok })}</span>
                        )}
                        {suggestion.parameters.context_window !== undefined && suggestion.parameters.context_window !== null && (
                          <span className="text-sm text-text-muted">{t('providers.models.context', { size: formatContext(suggestion.parameters.context_window, locale) })}</span>
                        )}
                        {offered.length > 0 && <span className="text-sm text-text-subtle">{t('models.list.offered', { list: offered.join(', ') })}</span>}
                      </span>
                    }
                    checked={chosen.includes(suggestion.model_name)}
                    disabled={accept.isPending}
                    onCheckedChange={(on) => setChosen((before) => (on ? [...before, suggestion.model_name] : before.filter((name) => name !== suggestion.model_name)))}
                  />
                </li>
              );
            })}
          </ul>
          <p className="text-sm text-text-muted">{t('models.list.suggestionsOffer')}</p>
          <div className="flex flex-wrap gap-2">
            <Button admin={adminSpec(provider, 'provider.manage')} variant="primary" pending={accept.isPending} blocked={chosen.length === 0} onClick={() => accept.mutate([...chosen])}>
              {t('providers.models.addSelected')}
            </Button>
            <Button admin={adminSpec(provider, 'provider.manage')} pending={accept.isPending} onClick={() => accept.mutate(suggestions.map((suggestion) => suggestion.model_name))}>
              {t('providers.models.addAll')}
            </Button>
          </div>
          {accept.error !== null && <ErrorAlert error={accept.error} />}
        </div>
      )}
    </Section>
  );
}

export function ProviderModels({ provider, manage }: { provider: Provider; manage: boolean }) {
  const { t } = useTranslation();
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<ProviderModel | null>(null);
  const [removing, setRemoving] = useState<ProviderModel | null>(null);
  const columns: Column<ProviderModel>[] = [
    {
      id: 'display',
      grow: true,
      header: t('providers.models.displayName'),
      className: 'min-w-[10rem]',
      cell: (model, shown) => {
        // What a column that is left out of a narrow place would have said is said under the name instead.
        const withoutRouting = !shown.has('routing');
        const withoutParameters = !shown.has('parameters');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <DisplayName provider={provider} model={model} manage={manage} />
            <ModelAvailability provider={provider} name={model.model_name} />
            {withoutRouting && model.display_name !== model.model_name && <code className="text-sm wrap-anywhere text-text-muted">{model.model_name}</code>}
            {model.groups.length > 0 && (
              <ul aria-label={t('models.groups.of')} className="flex flex-wrap gap-1">
                {model.groups.map((name) => (
                  <li key={name}>
                    <GroupChip name={name} />
                  </li>
                ))}
              </ul>
            )}
            {withoutParameters && <Summary parameters={model.parameters} />}
          </div>
        );
      },
    },
    { id: 'routing', header: t('providers.models.routingName'), from: 'md', cell: (model) => model.display_name === model.model_name ? null : <code className="text-sm wrap-anywhere">{model.model_name}</code> },
    { id: 'enabled', header: t('providers.models.enabled'), cell: (model) => <EnabledSwitch provider={provider} model={model} manage={manage} /> },
    { id: 'parameters', header: t('providers.models.parameters'), from: 'lg', cell: (model) => <Summary parameters={model.parameters} /> },
    { id: 'source', header: t('providers.models.source'), from: 'lg', className: 'text-text-muted', cell: (model) => t(`providers.models.sources.${model.source}`) },
    { id:'menu',header:t('providers.models.actions'),hideHeader:true,align:'end',cell:(model)=><Menu
      label={t('providers.models.menuOf',{name:shownName(model,t)})} align="end"
      entries={[
        ...(manage?[{id:'edit',label:t('models.list.edit'),icon:<ActionIcon action="edit" />,onSelect:()=>setEditing(model)}]:[]),
        {id:'switch-threads',label:t('resources.act.switchThreads'),icon:<ArrowRightLeft aria-hidden />,onSelect:()=>openChange({kind:'model',providerId:provider.id,id:model.id,name:shownName(model,t)},{kind:'switch'})},
        ...(manage?[{type:'separator' as const,id:'apart'},{id:'remove',label:t('resources.act.delete'),icon:<ActionIcon action="delete" />,danger:true,onSelect:()=>setRemoving(model)}]:[]),
      ]} trigger={<IconButton action="actions" size="sm" label={t('providers.models.menuOf',{name:shownName(model,t)})} />} />},
  ];

  return (
    <div className="flex flex-col gap-6">
      <Section
        id="provider-models"
        title={t('providerRecovery.savedModels')}
        action={
          manage ? (
            <Button variant="primary" icon={<Plus aria-hidden />} onClick={() => setAdding(true)}>
              {t('providers.models.add')}
            </Button>
          ) : undefined
        }
      >
        <div className="flex flex-col gap-4">
          <GroupStrip provider={provider} models={provider.models} manage={manage} />
          {provider.models.length === 0 ? (
            <EmptyState compact icon={<Plus aria-hidden />} title={t('providers.models.none')} description={manage ? t('providers.models.noneLead') : undefined} />
          ) : (
            <Table label={t('providers.models.title')} columns={columns} rows={provider.models} rowKey={(model) => model.id} />
          )}
        </div>
      </Section>
      {manage && <Suggestions provider={provider} />}
      {adding && <AddModelDialog provider={provider} onClose={() => setAdding(false)} />}
      {editing !== null && <ModelEditorDialog provider={provider} model={editing} onClose={() => setEditing(null)} />}
      {removing!==null && <ModelLifecycle key={removing.id} provider={provider} model={removing} kind="delete" onClose={()=>setRemoving(null)} />}
    </div>
  );
}
