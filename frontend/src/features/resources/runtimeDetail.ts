import type { TFunction } from 'i18next';

/** Durable reasons of a provider's runtime have translated words on both runtime lists and on the provider's page; the words of a host or a driver are shown as they are. */
export function runtimeDetail(detail: string | null, t: TFunction): string | null {
  if (detail === 'provider_idle_stop') return t('providerRecovery.runtimeStoppedIdle');
  if (detail === 'provider_person_stop') return t('providerRecovery.runtimeStoppedByPerson');
  if (detail === 'provider_forced_stop') return t('providerRecovery.runtimeStoppedByForce');
  if (detail === 'provider_update_ended') return t('providerRecovery.runtimeUpdateEnded');
  if (detail === 'provider_unit_unfinished') return t('providerRecovery.runtimeUnfinished');
  return detail;
}
