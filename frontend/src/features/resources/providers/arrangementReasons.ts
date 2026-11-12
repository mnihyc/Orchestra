/** The same native consent fields describe owner decisions on a provider and defaults in setup. */
import type { TFunction } from 'i18next';
import type { ArrangementConsent, ArrangementType } from '@/generated/api-types';

/** Saved providers report owner consent and their keeper's credential policy. */
export function providerArrangementReason(consent: ArrangementConsent, t: TFunction): string | null {
  if (!consent.allowed) return t('arrangementAvailability.owner');
  return consent.available ? null : t('arrangementAvailability.machine');
}

/** Setup's allowed flag is a proposed default; availability comes from the program and keeper. */
export function setupArrangementReason(consent: ArrangementConsent, machine: readonly ArrangementType[] | null | undefined, t: TFunction): string | null {
  if (!consent.available) return t('arrangementAvailability.program');
  return machine !== undefined && machine !== null && !machine.includes(consent.type)
    ? t('arrangementAvailability.machine') : null;
}
