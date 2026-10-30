/** Display names belong on work screens; routing names remain request values and configuration data. */
import type { TFunction } from 'i18next';
import { i18n } from '@/i18n';
import { offeredChoice } from '@/i18n/choices';

export function modelDisplayName(display: string | null | undefined, t: TFunction = i18n.t): string {
  // These are the built-in demonstration adapter's old labels, not vendor model identifiers.
  const legacy = display?.trim().toLowerCase();
  if (legacy === 'mock-large' || legacy === 'mock large') return t('composer.model.exampleLarge');
  if (legacy === 'mock-small' || legacy === 'mock small') return t('composer.model.exampleSmall');
  if (legacy === 'fake') return t('composer.model.example');
  return display === null || display === undefined || display.trim() === '' ? t('composer.model.displayUnknown') : display;
}

/** Built-in effort keys have catalogue terms; a plugin's own term remains its offered name. */
export function effortDisplayName(value: string, t: TFunction = i18n.t): string {
  return offeredChoice(t, 'effort', value)?.name ?? value;
}
