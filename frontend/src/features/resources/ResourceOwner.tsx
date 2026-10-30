/** A first owner is a person; an unowned place resource belongs to its home, otherwise the system. */
import { useTranslation } from 'react-i18next';
import { PlaceLink } from '@/features/navigation/places';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import type { NodeRef, UserRef } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';

export interface ResourceOwnership { owner: UserRef | null; home?: NodeRef | null }

function HomeOwner({ home }: { home: NodeRef }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  return <PlaceLink place={home}>{label(home) ?? t('access.place.hidden')}</PlaceLink>;
}

export function ResourceOwner({ owner, home }: ResourceOwnership) {
  const { t } = useTranslation();
  return <span data-resource-owner className="inline-flex min-w-0 items-center gap-1 text-sm text-text-muted">
    {owner !== null ? <UserChip user={owner} size={24} />
      : home !== null && home !== undefined ? <HomeOwner home={home} /> : t('resourceDetails.system')}
  </span>;
}
