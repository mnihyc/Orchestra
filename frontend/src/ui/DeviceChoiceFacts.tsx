/** Device facts reuse one opening on the actual current choice; values come from its own renderer. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { requestReader } from '@/lib/readAttempts';
import { deviceChoiceOf, type DeviceChoiceObservation, type DeviceChoiceFailure } from '@/lib/viewChoiceFacts';
import { FactDisclosure, FactDisclosureBody, type FactGroup } from '@/ui/FactDisclosure';

type DeviceChoiceDetails = Partial<DeviceChoiceObservation> & Pick<DeviceChoiceObservation, 'reader' | 'origin' | 'saved' | 'saved_choice_availability' | 'read_result' | 'write_result'>;

export function useDeviceChoiceGroups(observation: DeviceChoiceDetails | undefined, current: ReactNode, saved?: ReactNode) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state) => requestReader(state.userId, state.readerVersion));
  const user = useStore(client.session, (state) => state.userId);
  const fact = observation?.reader === reader && user !== null ? observation : undefined;
  const fragment = fact === undefined ? undefined : fact.write_result === 'failed' ? t('clientFacts.device.notSaved')
    : (fact.origin === 'default' || fact.read_result === 'invalid') && fact.saved_choice_availability === 'unavailable' ? t('clientFacts.device.unavailableChoice')
      : fact.read_result === 'unavailable' ? t('clientFacts.device.unavailableRead') : undefined;
  const groups: FactGroup[] = fact === undefined ? [] : [{ key: 'device-choice', facts: [
    { key: 'source', label: t('clientFacts.device.source'), value: t(`clientFacts.device.origins.${fact.origin}`) },
    { key: 'current', label: t('clientFacts.device.current'), value: current },
    ...(fact.saved === undefined && fact.saved_choice_availability !== 'unavailable' ? [] : [{ key: 'saved', label: t('clientFacts.device.saved'), value: saved }]),
    ...(fact.read_result === 'unavailable' ? [{ key: 'read', label: t('common.reason'), value: t('clientFacts.device.unavailableRead') }]
      : fact.saved_choice_availability === 'unavailable' ? [{ key: 'availability', label: t('common.reason'), value: t('clientFacts.device.savedUnavailable') }] : []),
    { key: 'save', label: t('clientFacts.device.save'), value: t(`clientFacts.device.saves.${fact.write_result}`) },
  ] }];
  return { groups, fragment };
}

export function DeviceChoiceFacts({ observation, current, saved, title, inline = false }: {
  observation?: DeviceChoiceObservation; current: ReactNode; saved?: ReactNode; title?: ReactNode; inline?: boolean;
}) {
  const { groups: facts, fragment } = useDeviceChoiceGroups(observation, current, saved);
  const groups = title === undefined ? facts : facts.map((group) => ({ ...group, title }));
  if (groups.length === 0) return null;
  return inline ? <FactDisclosureBody groups={groups} />
    : <FactDisclosure groups={groups} fragment={fragment} />;
}

/** An existing picker owns the opening; only its actual exception appears beside the closed value. */
export function DeviceChoiceException({ observation }: { observation: DeviceChoiceObservation }) {
  const { fragment } = useDeviceChoiceGroups(observation, undefined);
  return fragment === undefined ? null : <span className="text-sm text-text-muted" data-device-choice-exception>{fragment}</span>;
}

export function DeviceToggleFacts({ writer, checked, inline = false }: { writer: object; checked: boolean; inline?: boolean }) {
  const { t } = useTranslation();
  const observation = deviceChoiceOf(writer);
  if (observation === undefined) return null;
  const name = (value: unknown) => typeof value === 'boolean' ? t(value ? 'common.on' : 'common.off') : undefined;
  return <DeviceChoiceFacts observation={{ ...observation, effective: checked }} current={name(checked)} saved={name(observation.saved)} inline={inline} />;
}

export function DeviceChoiceWriterException({ writer }: { writer: object }) {
  const observation = deviceChoiceOf(writer);
  return observation === undefined ? null : <DeviceChoiceException observation={observation} />;
}

/** Native disclosures reuse their existing open body, rather than adding another opening in the summary. */
export function DeviceExpansionFacts({ writer, open }: { writer: object; open: boolean }) {
  const { t } = useTranslation();
  const observation = deviceChoiceOf(writer);
  if (observation === undefined) return null;
  const name = (value: unknown) => typeof value === 'boolean' ? t(value ? 'clientFacts.device.open' : 'clientFacts.device.closed') : undefined;
  return <DeviceChoiceFacts observation={{ ...observation, effective: open }} current={name(open)} saved={name(observation.saved)} inline />;
}

/** Disappeared choices expose only the safe observed save result, without pretending bytes were stored. */
export function DeviceChoiceFailureFacts({ failure }: { failure: DeviceChoiceFailure }) {
  const { t } = useTranslation();
  return <FactDisclosure fragment={t('clientFacts.device.notSaved')} groups={[{ key: 'device-save', facts: [
    { key: 'source', label: t('clientFacts.device.source'), value: t('clientFacts.device.origins.page') },
    { key: 'save', label: t('clientFacts.device.save'), value: t(`clientFacts.device.saves.${failure.write_result}`) },
  ] }]} />;
}
