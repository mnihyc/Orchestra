/** The empty place's creation question opens its actual permission row, without changing the page. */
import { useTranslation } from 'react-i18next';

import type { AccessTarget } from '@/api/access';
import { openAccess } from '@/features/access/AccessDialog';
import { Button } from '@/ui/Button';

export function CreationPermissionLink({ place }: { place: AccessTarget & { kind: 'org' | 'project' } }) {
  const { t } = useTranslation();
  const open = () => openAccess(place, { share: false, permission: place.kind === 'project' ? 'project.create_topics' : 'org.interact' });
  return <Button action="help" size="sm" variant="ghost" onClick={open}>{t(place.kind === 'org' ? 'creationPermissions.projects' : 'creationPermissions.topics')}</Button>;
}
