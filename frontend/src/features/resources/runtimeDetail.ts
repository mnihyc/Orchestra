import type { TFunction } from 'i18next';

/** Durable provider stop reasons have translated words on both runtime lists. */
export function runtimeDetail(detail: string | null, t: TFunction): string | null {
  if (detail === 'provider_idle_stop') return t('providerRecovery.runtimeStoppedIdle');
  if (detail === 'provider_person_stop') return t('providerRecovery.runtimeStoppedByPerson');
  return detail;
}
