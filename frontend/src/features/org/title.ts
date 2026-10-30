/** The same organization purpose in a page heading and its frame or tab. */
import type { TFunction } from 'i18next';
import type { Org } from '@/generated/api-types';

export function orgPageTitle(org: Pick<Org, 'name' | 'org_kind'>, t: TFunction): string {
  return org.org_kind === 'personal' ? t('places.org.personalTitle', { name: org.name }) : org.name;
}
