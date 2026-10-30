/** Secondary org-page acts share the existing lazy place menu; only the star and primary act stay outside. */
import { useTranslation } from 'react-i18next';
import { NodeMenuButton } from '@/features/navigation/NodeMenu';
import { usePlaceHeaderPanels } from '@/features/navigation/PlaceHeaderPanels';
import { placeOfNode } from '@/features/navigation/places';
import type { Org } from '@/generated/api-types';

export function OrgHeaderMenu({ org }: { org: Org }) {
  const { t } = useTranslation();
  const panels = usePlaceHeaderPanels(org, ['details', 'announcements', 'notifications'], t('notificationBell.place'));
  return <>
    <NodeMenuButton place={placeOfNode(org)} node={org} where="screen" extraEntries={panels.entries} triggerRef={panels.opener} />
    {panels.dialogs}
  </>;
}
