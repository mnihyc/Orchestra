/** The ordinary topic menu, with its status acts using captured revisions and a confirmed Undo. */
import { Ellipsis } from 'lucide-react';
import type { Ref } from 'react';
import { useTranslation } from 'react-i18next';

import { useIsPending, useNodeMenu } from '@/features/navigation/NodeMenu';
import { usePlaceHeaderPanels } from '@/features/navigation/PlaceHeaderPanels';
import { placeOfNode } from '@/features/navigation/places';
import { useStatusChange } from '@/features/topic/statusUndo';
import type { Topic } from '@/generated/api-types';
import { ContextEntries } from '@/ui/ContextMenu';
import { IconButton } from '@/ui/IconButton';
import { DropdownEntries, Menu, type MenuEntry } from '@/ui/Menu';
import { useErrorText } from '@/ui/errorText';
import { toast } from '@/ui/Toast';

/** Both menu surfaces read one entry list; the other ordinary topic acts keep their existing requests. */
export function TopicMenuEntries({ topic, where = 'row', context = false, extraEntries = [] }: { topic: Topic; where?: 'row' | 'screen'; context?: boolean; extraEntries?: readonly MenuEntry[] }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const busy = useIsPending(topic.id);
  const change = useStatusChange();
  const ordinary = useNodeMenu(placeOfNode(topic), { where, node: topic });
  const entries = ordinary.map((entry): MenuEntry => {
    if (entry.type !== undefined && entry.type !== 'item') return entry;
    if (entry.id !== 'done' && entry.id !== 'reopen') return entry;
    const status = entry.id === 'done' ? 'done' : 'open';
    return { ...entry, disabled: busy ? t('places.menu.working') : entry.disabled, onSelect: () => {
      void change(topic, status).catch((error: unknown) => toast({ tone: 'danger', title: t('places.toast.notChanged', { name: topic.name }), description: describe(error).message }));
    } };
  });
  const all = extraEntries.length === 0 ? entries : [...entries, { type: 'separator' as const, id: 'header-panels' }, ...extraEntries];
  return context ? <ContextEntries entries={all} /> : <DropdownEntries entries={all} />;
}

export function TopicMenu({ topic, where = 'row', className, extraEntries, triggerRef }: { topic: Topic; where?: 'row' | 'screen'; className?: string; extraEntries?: readonly MenuEntry[]; triggerRef?: Ref<HTMLButtonElement> }) {
  const { t } = useTranslation();
  return <Menu label={t('places.menu.of', { name: topic.name })} align="end" content={<TopicMenuEntries topic={topic} where={where} {...(extraEntries === undefined ? {} : { extraEntries })} />} trigger={<IconButton ref={triggerRef} size="sm" label={t('places.menu.more', { name: topic.name })} icon={<Ellipsis aria-hidden />} className={className} />} />;
}

/** The topic page's "…": its menu, with a named entry for the Announcements and Notifications icons of its header. */
export function TopicHeaderMenu({ topic }: { topic: Topic }) {
  const { t } = useTranslation();
  const panels = usePlaceHeaderPanels(topic, ['announcements', 'notifications'], t('places.topic.bell'));
  return <><TopicMenu topic={topic} where="screen" extraEntries={panels.entries} triggerRef={panels.opener} />{panels.dialogs}</>;
}
