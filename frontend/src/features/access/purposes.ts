/** Provider purposes use the same short names in grants, requests and refusals. */
import type { TFunction } from 'i18next';

export const purposeName = (t: TFunction, value: string): string => t(`access.options.purpose.${value}`, { defaultValue: value });

export const purposeNames = (t: TFunction, values: readonly string[]): string => values.length === 0
  ? t('access.options.noPurposes') : values.map((value) => purposeName(t, value)).join(', ');
