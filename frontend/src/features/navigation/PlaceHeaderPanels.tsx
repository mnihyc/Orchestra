/** The named counterparts of a place page's header icons: entries of its "…" menu that open its description, announcements or notifications in a dialog.
 *
 * A compact icon in a header keeps its act reachable by name, by keyboard and on touch: the same act stands in the
 * page's menu, and both open one dialog. Org, project and topic pages read it from here.
 */
import { Bell, FileText, Megaphone } from 'lucide-react';
import { type ReactNode, type RefObject, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { PlaceAnnouncementContents } from '@/features/announcements/Composer';
import { PlaceDetailsContents } from '@/features/navigation/PlaceDetails';
import { SubscriptionBell } from '@/features/thread/SubscriptionBell';
import type { Org, Project, Topic } from '@/generated/api-types';
import { Dialog } from '@/ui/Dialog';
import type { MenuEntry } from '@/ui/Menu';

/** What a place page's header shows as an icon and its menu names. */
export type PlaceHeaderPanel = 'details' | 'announcements' | 'notifications';

const IDS: Record<PlaceHeaderPanel, string> = { details: 'description', announcements: 'announcements', notifications: 'notifications' };

/** The menu entries for `panels` and the dialogs they open; `bell` is what this kind of place calls its notifications. */
export function usePlaceHeaderPanels(node: Org | Project | Topic, panels: readonly PlaceHeaderPanel[], bell: string): {
  entries: MenuEntry[]; dialogs: ReactNode; opener: RefObject<HTMLButtonElement | null>;
} {
  const { t } = useTranslation();
  const [open, setOpen] = useState<PlaceHeaderPanel | null>(null);
  const opener = useRef<HTMLButtonElement>(null);
  const show = (next: PlaceHeaderPanel) => {
    // The chosen item goes with its menu; the dialog gives focus back to the page's lasting menu button.
    opener.current?.focus();
    setOpen(next);
  };
  const close = () => setOpen(null);
  const words: Record<PlaceHeaderPanel, { label: string; icon: ReactNode }> = {
    details: { label: t('places.details.title'), icon: <FileText aria-hidden /> },
    announcements: { label: t('placeAnnouncements.title'), icon: <Megaphone aria-hidden /> },
    notifications: { label: bell, icon: <Bell aria-hidden /> },
  };
  // A topic has no description panel of its own: its description stands under its title.
  const offered = panels.filter((panel) => panel !== 'details' || node.kind !== 'topic');
  const entries = offered.map((panel): MenuEntry => ({ id: `${node.kind}-${IDS[panel]}`, ...words[panel], onSelect: () => show(panel) }));
  const dialogs = <>
    {open === 'details' && node.kind !== 'topic' && <Dialog open onOpenChange={close} title={t('places.details.title')}>
      <PlaceDetailsContents node={node} onEdit={close} />
    </Dialog>}
    {open === 'announcements' && <Dialog open onOpenChange={close} title={t('placeAnnouncements.at', { name: node.name })}>
      <PlaceAnnouncementContents place={node} />
    </Dialog>}
    {open === 'notifications' && <Dialog open onOpenChange={close} title={bell}>
      <SubscriptionBell thread={node} label={bell} />
    </Dialog>}
  </>;
  return { entries, dialogs, opener };
}
