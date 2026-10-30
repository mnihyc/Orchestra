/** A collection explains editing authority once; individual values keep just their source. */
import { useTranslation } from 'react-i18next';
import type { UserRef } from '@/generated/api-types';

export function ReadOnlySettingsNotice({ owner, partial = false }: { owner?: Pick<UserRef, 'display_name'> | null; partial?: boolean }) {
  const { t } = useTranslation();
  return <p data-read-only-settings className="text-sm text-text-muted">{owner === null || owner === undefined
    ? t(partial ? 'settings.compact.someViewOnly' : 'settings.compact.viewOnly') : t(partial ? 'settings.compact.someViewOnlyOwner' : 'settings.compact.viewOnlyOwner', { person: owner.display_name })}</p>;
}
