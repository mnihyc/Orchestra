/** Ask only for the missing required choice.
 *
 * Where the defaults of a place do not settle what a thread there runs on, the inline starter does not open a dialog:
 * it says which choice is missing and asks for that one alone, a provider, a profile or a model, where the person is
 * typing. Where the person may write the defaults of a place (it takes managing it), the choice can be kept for the
 * topic, the project or the org, the widest of them ticked, and is written before anything is made; where they may
 * not, the same place says so and offers to ask for the right. A profile whose harness has not reported what it can
 * do is said to be waiting for that, with nothing offered that cannot run, and what is offered returns by itself when
 * the report comes. What no choice here can mend (no environment may be used or made, no arrangement can be had) says so,
 * with the way on.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Info, LoaderCircle, TriangleAlert } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption, offeredChoice } from '@/i18n/choices';

import { nodeQuery } from '@/api/nodes';
import { providersQuery } from '@/api/resources';
import { setStartDefault } from '@/api/start';
import { AccessRequest } from '@/features/access/AccessRequest';
import { providerNamesCollide, providerPickerLabels } from '@/features/resources/providers/labels';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { programNameIn } from '@/features/resources/programNames';
import { ResourceCreationActions } from '@/features/resources/ResourceCreation';
import type { PlaceRef } from '@/features/navigation/places';
import type { Project, Topic } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Select } from '@/ui/Select';
import { cx } from '@/ui/cx';

import { type Missing, modelName, type Picks, type Resolved, settingsToKeep } from './choices';
import type { StartOptions } from '@/api/start';
import { RequestDefault } from './RequestDefault';
import { agentWhy, arrangementWhy, providerWhy } from './words';

/** A place whose defaults decide what the threads made in the place run on, and whether the person may write them there. */
export interface Scope {
  id: string;
  kind: 'topic' | 'project' | 'org';
  name: string;
  may: boolean;
}

/**
 * The places a default can be written at for the threads made in a topic or in a project, the narrowest first: the place itself,
 * the project a topic lies in, and the org it lies in; each with whether the person may write there. Writing a default is a
 * parameter of the place, which is its manager's to set.
 */
export function useScopes(node: Topic | Project): Scope[] {
  const org = [...node.path].reverse().find((entry) => entry.kind === 'org') ?? null;
  const project = node.kind === 'topic' ? node.parent_id : null;
  const above = useQuery({ ...nodeQuery(project ?? ''), enabled: project !== null });
  const orgNode = useQuery({ ...nodeQuery(org?.id ?? ''), enabled: org !== null });
  return [
    { id: node.id, kind: node.kind, name: node.name, may: holds(node.access, `${node.kind}.manage`) },
    ...(project === null ? [] : [{ id: project, kind: 'project' as const, name: above.data?.name ?? '', may: holds(above.data?.access, 'project.manage') }]),
    ...(org === null ? [] : [{ id: org.id, kind: 'org' as const, name: org.name ?? '', may: holds(orgNode.data?.access, 'org.manage') }]),
  ];
}

/** The scope a choice is kept at when the person has not said: the widest one they may write. */
export function widestScope(scopes: readonly Scope[]): Scope | null {
  return [...scopes].reverse().find((scope) => scope.may) ?? null;
}

/** The first thing in the way that a person could settle here by choosing: a provider, a profile or a model. */
function firstChoice(missing: readonly Missing[]): Missing | null {
  return missing.find((entry) => entry.kind === 'provider' || entry.kind === 'agent' || entry.kind === 'model') ?? null;
}

export interface MissingDefaultProps {
  place: PlaceRef;
  node: Topic | Project;
  options: StartOptions;
  resolved: Resolved;
  picks: Picks;
  onPick: (patch: Partial<Picks>) => void;
  scopes: readonly Scope[];
  /** Where the choice is kept: a place's id, or `null` for this thread alone. */
  keepAt: string | null;
  onKeepAt: (id: string | null) => void;
  /** Opens the dialog that chooses everything. */
  onChange: () => void;
}

export function MissingDefault({ place, node, options, resolved, picks, onPick, scopes, keepAt, onKeepAt, onChange }: MissingDefaultProps) {
  const { t, i18n } = useTranslation();
  const [asking, setAsking] = useState(false);
  const choice = firstChoice(resolved.missing);
  const providerRefs = resolved.providers.map((row) => row.entry.provider);
  const needProviderContext = choice !== null && (choice.kind === 'provider' || picks.provider !== null) && providerNamesCollide(providerRefs);
  const providers = useQuery({ ...providersQuery('usable'), enabled: needProviderContext, refetchOnMount: 'always', retry: false });
  const places = useKnownPlaces({ trees: false });
  const missingProviderContext = needProviderContext && (!providers.isFetchedAfterMount || providers.isError);
  const providerNames = providerPickerLabels(providerRefs, missingProviderContext ? [] : providers.data ?? [], places);
  const mayWrite = scopes.some((scope) => scope.may);
  const keeping = keepAt === null ? [] : settingsToKeep(options, resolved);
  const keep = useMutation({
    mutationFn: async () => {
      for (const setting of keeping) await setStartDefault(keepAt ?? '', setting.key, setting.value);
    },
    onSuccess: () => {
      // What was written is the place's default now: the answer says so, and the picks that stood for it are not needed.
      onPick({ provider: null, agent: null, model: null, effort: null });
    },
  });

  const sentence = ((): string | null => {
    const first = resolved.missing[0];
    // Everything is chosen, by the person: it is for this thread, and may be kept for the place.
    if (first === undefined) return keeping.length > 0 ? t('start.missing.chosen') : null;
    const agent = resolved.agents.find((row) => row.isDefault) ?? resolved.agents.find((row) => row.unreported) ?? resolved.agents[0];
    const providerName = resolved.provider===null?'':providerNames.get(resolved.provider.id) ?? resolved.provider.entry.provider.name;
    switch (first.kind) {
      case 'provider':
        if (first.none) return t('start.missing.providerNone');
        if (first.unusable !== null) return t('start.missing.providerUnusable', { provider: providerNames.get(first.unusable.id) ?? first.unusable.entry.provider.name, why: providerWhy(first.unusable, null, t) ?? '' });
        return t('start.missing.provider', { place: place.name });
      case 'agent':
        if (first.unreported) return t('start.missing.agentUnreported', { agent: agent?.entry.agent.name ?? '' });
        if (first.none) return t('start.missing.agentNone', { provider: providerName });
        if (first.unusable !== null) return t('startPreflight.agentUnavailable', { agent: first.unusable.entry.agent.name, why: agentWhy(first.unusable, t) });
        if (agent !== undefined && !resolved.agents.some((row) => row.startable)) return t('startPreflight.agentUnavailable', { agent: agent.entry.agent.name, why: agentWhy(agent, t) });
        return t('start.missing.agent', { place: place.name });
      case 'model':
        return first.none ? t('start.missing.modelNone', { provider: providerName }) : t('start.missing.model', { place: place.name });
      case 'environment':
        return t('start.missing.environment');
      case 'arrangement':
        return t('start.missing.arrangement', { agent: resolved.agent?.entry.agent.name ?? '', provider: providerName, why: first.first === null ? t('start.arrangement.unknownShort') : arrangementWhy(first.first.entry, t) });
    }
  })();
  if (sentence === null) return null;

  const first = resolved.missing[0];
  const waiting = resolved.waiting;
  const chosenProvider = resolved.provider;
  const whole = first === undefined;
  // What a choice settles is asked quietly; what no choice here can settle (no environment, no way of placing the agent) is a warning.
  const stuck = first?.kind === 'environment' || first?.kind === 'arrangement';

  const keepRow =
    (choice !== null || whole) && (!waiting || resolved.agents.some((row) => row.startable)) ? (
      mayWrite ? (
        <>
          <Select
            label={t('start.missing.keepFor')}
            value={keepAt ?? 'none'}
            onValueChange={(next) => onKeepAt(next === 'none' ? null : next)}
            options={[
              ...scopes.filter((scope) => scope.may).map((scope) => ({ value: scope.id, label: scope.kind === 'org' ? t('start.missing.scopeOrg', { name: scope.name }) : t(`start.missing.scope.${scope.kind}`) })),
              { value: 'none', label: t('start.missing.keepNone') },
            ]}
            fieldClassName="min-w-(--field-narrow)"
          />
          <Button size="sm" pending={keep.isPending} blocked={keeping.length === 0} onClick={() => keeping.length > 0 && keep.mutate()}>
            {t('start.missing.keep')}
          </Button>
        </>
      ) : (
        !whole && (
          <>
            <span className="text-text-muted">{t('start.missing.mayNot', { place: place.name })}</span>
            <Button size="sm" variant="secondary" onClick={() => setAsking(true)}>
              {t('start.missing.request')}
            </Button>
          </>
        )
      )
    ) : null;

  return (
    <div className={cx('flex flex-col gap-3 rounded-lg border px-3 py-3 text-sm text-text', stuck ? 'border-warning/40 bg-warning-subtle' : 'border-border bg-bg-subtle')} data-missing-default role="group" aria-label={t('start.missing.label')}>
      <p className="flex items-start gap-2" role="status">
        {waiting ? <LoaderCircle className="mt-0.5 animate-spin text-text-subtle" aria-hidden /> : stuck ? <TriangleAlert className="mt-0.5 text-warning" aria-hidden /> : <Info className="mt-0.5 text-info" aria-hidden />}
        <span>{sentence}</span>
      </p>

      {choice !== null && (
        <div className="flex flex-wrap items-start gap-3">
          {(choice.kind === 'provider' || picks.provider !== null) && (
            <Select
              label={missingProviderContext ? t(providers.isError ? 'startFlow.providerLocationsFailure' : 'startFlow.providerLocationsLoading') : t('start.provider.title')}
              value={chosenProvider?.id ?? ''}
              placeholder={t('start.missing.pickProvider')}
              disabled={options.defaults.locked || missingProviderContext}
              onValueChange={(next) => onPick({ provider: next, agent: null, model: null, effort: null, arrangement: null })}
              options={resolved.providers.filter((row) => row.choosable).map((row) => ({ value: row.id, label: providerNames.get(row.id) ?? row.entry.provider.name }))}
              fieldClassName="min-w-(--field-narrow) flex-1"
            />
          )}
          {chosenProvider !== null && resolved.agents.some((row) => row.startable) && (choice.kind === 'agent' || picks.agent !== null) && (
            <Select
              label={t('start.agent.title')}
              value={resolved.agent?.id ?? ''}
              placeholder={t('start.missing.pickAgent')}
              onValueChange={(next) => onPick({ agent: next, arrangement: null })}
              options={resolved.agents.filter((row) => row.startable).map((row) => {
                const program = programNameIn(row.entry.agent, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
                return { value: row.id, label: row.entry.agent.name, description: program === row.entry.agent.name || program === t('resources.harnessNameUnknown') ? undefined : program };
              })}
              fieldClassName="min-w-(--field-narrow) flex-1"
            />
          )}
          {chosenProvider !== null && (choice.kind === 'model' || picks.model !== null) && (
            <>
              <Select
                label={t('start.model.model')}
                value={resolved.model?.model_name ?? ''}
                placeholder={t('start.missing.pickModel')}
                onValueChange={(next) => onPick({ model: next, effort: null })}
                options={resolved.models.map((model) => ({ value: model.model_name, label: modelName(model) }))}
                fieldClassName="min-w-(--field-narrow) flex-1"
              />
              {resolved.model !== null && resolved.efforts.length > 0 && (
                <Select label={t('start.model.effort')} value={resolved.effort ?? ''} onValueChange={(next) => onPick({ effort: next })} options={resolved.efforts.map((value) => ({ value, ...(offeredChoice(t, 'effort', value) ?? choiceOption(t, 'dynamic.value', { name: value, context: t('start.model.effort') })) }))} fieldClassName="min-w-(--field-narrow)" />
              )}
            </>
          )}
        </div>
      )}

      {(resolved.providers.every((row) => !row.choosable) || resolved.agents.every((row) => !row.startable)) && <ResourceCreationActions onReadChoices={onChange} />}
      {first?.kind === 'environment' && (
        <details>
          <summary className="cursor-default rounded-md select-none hover:text-text-muted">{t('start.where.mayNotMake')}</summary>
          <div className="mt-2">
            <AccessRequest missing={options.can_create.missing} objectId={options.can_create.missing[0]?.object.id ?? place.id} fixed />
          </div>
        </details>
      )}

      <div className="flex flex-wrap items-end gap-x-3 gap-y-2">
        {keepRow}
        <Button size="sm" variant="ghost" className="ml-auto" onClick={onChange}>
          {t('start.missing.choose')}
        </Button>
      </div>
      {needProviderContext && providers.isError && <ErrorAlert error={providers.error} message={t('startFlow.providerLocationsFailure')} action={<Button size="sm" pending={providers.isFetching} onClick={() => void providers.refetch()}>{t('common.tryAgain')}</Button>} />}
      {keep.error !== null && <ErrorAlert error={keep.error} />}
      {asking && <RequestDefault place={place} owner={node.owner} open onClose={() => setAsking(false)} />}
    </div>
  );
}
