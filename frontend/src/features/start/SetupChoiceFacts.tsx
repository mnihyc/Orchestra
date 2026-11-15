/** The current native resolution and its saved server choice share one opening beside the setup. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import type { StartOptions } from '@/api/start';
import { modelPurpose } from '@/features/models/purpose';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { offeredChoice } from '@/i18n/choices';
import { requestReader } from '@/lib/readAttempts';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import type { Picks, Resolved } from './choices';
import type { LastSetupObservation } from './lastSetup';

export function SetupChoiceFacts({ options, resolved, observation }: { options: StartOptions | null; resolved: Resolved | null; observation?: LastSetupObservation }) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => requestReader(state.userId, state.readerVersion));
  const user = useStore(client.session, (state) => state.userId);
  if (options === null || resolved === null || observation === undefined || observation.reader !== reader || user === null) return null;
  const saved = (field: keyof Picks): string | undefined => {
    const value: unknown = typeof observation.saved === 'object' && observation.saved !== null ? Reflect.get(observation.saved, field) : undefined;
    return typeof value === 'string' ? value : undefined;
  };
  const provider = options.providers.find((row) => row.provider.id === saved('provider'));
  const agent = provider?.agents.find((row) => row.agent.id === saved('agent'));
  const model = provider?.models.find((row) => row.model_name === saved('model'));
  const environment = options.environments.find((row) => row.environment.id === saved('environment'));
  const arrangement = resolved.arrangements.find((row) => row.type === saved('arrangement'));
  const field = (key: keyof Resolved['origins'], title: string, current: ReactNode, previous: ReactNode, available: boolean): FactGroup => ({ key: `setup-${key}`, title, facts: [
    { key: 'source', label: t('clientFacts.device.source'), value: resolved.origins[key] === 'last_used' ? t('startImplicit.savedSetup') : t(`clientFacts.device.origins.${resolved.origins[key] === null ? 'page' : 'default'}`) },
    { key: 'current', label: t('clientFacts.device.current'), value: current },
    ...(saved(key) === undefined ? [] : [{ key: 'saved', label: t('clientFacts.device.saved'), value: previous }]),
    ...(saved(key) === undefined || available ? [] : [{ key: 'availability', label: t('common.reason'), value: t('clientFacts.device.savedUnavailable') }]),
  ] });
  const resource = (kind: 'provider' | 'agent' | 'environment', value: { id: string; name: string } | undefined): ReactNode => value === undefined ? undefined
    : kind === 'provider' ? <ResourceChip subject={{ kind, provider: value }} hint={false} />
      : kind === 'agent' ? <ResourceChip subject={{ kind, agent: value }} hint={false} /> : <ResourceChip subject={{ kind, environment: value }} hint={false} />;
  const modelName = (value: typeof model, providerId: string | undefined): ReactNode => value === undefined || providerId === undefined ? undefined : <ResourceChip subject={{ kind: 'model', model: value, providerId }} hint={false} />;
  const groups = [
    field('provider', t('resourceFacts.provider'), resource('provider', resolved.provider?.entry.provider), resource('provider', provider?.provider), provider?.usable === true),
    field('agent', t('start.agent.title'), resource('agent', resolved.agent?.entry.agent), resource('agent', agent?.agent), agent?.usable === true),
    field('model', t('start.model.model'), modelName(resolved.model ?? undefined, resolved.provider?.id), modelName(model, provider?.provider.id), model?.allowed === true && modelPurpose(model, 'generation') !== 'unsupported'),
    field('effort', t('composer.effort.title'), resolved.effort === null ? undefined : offeredChoice(t, 'effort', resolved.effort)?.name, model?.parameters.efforts?.includes(saved('effort') ?? '') === true ? offeredChoice(t, 'effort', saved('effort') ?? '')?.name : undefined, model?.parameters.efforts?.includes(saved('effort') ?? '') === true),
    field('environment', t('start.where.title'), resolved.fresh ? t('start.quick.inNew') : resource('environment', resolved.environment?.environment), resource('environment', environment?.environment), environment?.usable === true),
    field('arrangement', t('start.arrangement.label'), resolved.arrangement === null ? undefined : t(`choiceCatalog.arrangement.${resolved.arrangement}.name`), arrangement === undefined ? undefined : t(`choiceCatalog.arrangement.${arrangement.type}.name`), arrangement?.open === true),
  ];
  const unavailable = observation.unavailable || groups.some((group) => group.facts.some((fact) => fact.key === 'availability'));
  const usesDefault = Object.values(resolved.origins).some((origin) => origin !== null && origin !== 'last_used');
  const fragment = observation.status === 'error' ? t('startImplicit.savedSetupReadFailed') : observation.unavailable ? t('clientFacts.device.savedUnavailable') : unavailable && usesDefault ? t('clientFacts.device.unavailableChoice') : undefined;
  return <FactDisclosure groups={groups} fragment={fragment} />;
}
