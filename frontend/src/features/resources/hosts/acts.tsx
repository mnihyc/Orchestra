/** What can be done with a host, from its row in the list and from its own page: pause it or resume it, open, edit, rename, copy its link and delete, each only where the reader may.
 *
 * A host is paused when it should take no new environments and resumed when it should take them again; what
 * runs on it goes on either way, and nothing here asks, since nothing is lost. Deleting asks for the host's name,
 * as the request does, and so is not an act that rows share. The same acts serve the button and the menu at the end
 * of a row, the menu under the right mouse button, the acts rows that are ticked share, and the menu and the Edit
 * button in the heading of the page.
 */
import { useResourceCleanup } from '@/features/storage/acts';
import { useNavigate } from '@tanstack/react-router';
import { CirclePause, CirclePlay, Copy, ArrowRight, ArrowRightLeft, TriangleAlert, Pencil, Recycle, TextCursorInput, Trash2 } from 'lucide-react';
import { type ReactNode, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { DeclareLostDialog } from '@/features/resources/DeclareLost';
import { updateHost } from '@/api/resources';
import { useComponentUpdateAct } from '@/features/resources/ComponentUpdate';
import { openChange } from '@/features/change/target';
import { type Act, type BulkAct, entriesOf, keysOf, type ListActs, type RowActs, useBusy, useCopyLink, shareAct } from '@/features/resources/acts';
import { EditHost } from '@/features/resources/hosts/Edit';
import { RemoveHostDialog } from '@/features/resources/hosts/Remove';
import type { Host } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { toast } from '@/ui/Toast';
import { adminSpec } from '@/ui/AdminAct';

export interface Asked {
  what: 'rename' | 'delete' | 'lost';
  host: Host;
}

export interface HostActs {
  list: ListActs<Host>;
  /** Asks for an act that has a dialog, for a button of the page that is not in the menu. */
  ask: (what: 'edit' | Asked['what'], host: Host) => void;
  page: (host: Host) => { edit: Act | null; primary: Act | null; menu: RowActs['entries'] };
  dialogs: ReactNode;
}

export function useHostActs(options: { afterDelete?: () => void; onSettings?: (host: Host) => void } = {}): HostActs {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const copyLink = useCopyLink();
  const { busy, track } = useBusy();
  const cleanup = useResourceCleanup();
  const [asked, setAsked] = useState<Asked | null>(null);
  const componentUpdate = useComponentUpdateAct();
  const editSettings = (host: Host) => {
    if (options.onSettings !== undefined) options.onSettings(host);
    else void navigate({ to: '/resources/hosts/$id', params: { id: host.id }, search: { tab: 'settings' } });
  };

  /** Lets a host take new environments, or not, on the revision that was read. */
  const setSchedulable = (host: Host, schedulable: boolean): Promise<Host> =>
    updateHost(host.id, { expected_revision: host.revision, schedulable });

  const actsOf = (host: Host, where: 'row' | 'page'): RowActs & { edit: Act | null } => {
    const manage = holds(host.access, 'host.manage');
    const updateNow = componentUpdate({kind:'host',id:host.id,update:host.software_update,manage,online:host.machine.online,admin:adminSpec(host,'host.manage')});
    const open: Act | null =
      where === 'row' ? { id: 'open', label: t('resources.act.open'), icon: <ArrowRight aria-hidden />, run: () => void navigate({ to: '/resources/hosts/$id', params: { id: host.id } }) } : null;
    const resume: Act | null =
      manage && host.lost_at === undefined && !host.schedulable
        ? {
            id: 'resume',
            label: t('resources.act.resume'),
            description: t('resources.act.resumeWhy'),
            icon: <CirclePlay aria-hidden />,
            run: () =>
              track(host.id, async () => {
                const updated = await setSchedulable(host, true);
                toast({ tone: 'success', title: t('hosts.settings.opened', { name: updated.name }) });
              }),
          }
        : null;
    const pause: Act | null =
      manage && host.schedulable
        ? {
            id: 'pause',
            label: t('resources.act.pause'),
            description: t('resources.act.pauseWhy'),
            icon: <CirclePause aria-hidden />,
            run: () =>
              track(host.id, async () => {
                const updated = await setSchedulable(host, false);
                toast({ tone: 'success', title: t('hosts.settings.closed', { name: updated.name }) });
              }),
          }
        : null;
    const edit: Act | null = manage ? { id: 'edit', label: t('resources.act.edit'), icon: <Pencil aria-hidden />, run: () => editSettings(host) } : null;
    const rename: Act | null = manage ? { id: 'rename', label: t('resources.act.rename'), icon: <TextCursorInput aria-hidden />, shortcut: 'F2', run: () => setAsked({ what: 'rename', host }) } : null;
    const share = shareAct(t, { kind: 'host', id: host.id, name: host.name }, host.access);
    const copy: Act = { id: 'copy-link', label: t('resources.act.copyLink'), icon: <Copy aria-hidden />, run: () => copyLink(`/resources/hosts/${host.id}`) };
    const del: Act | null = manage ? { id: 'delete', label: t('resources.act.delete'), icon: <Trash2 aria-hidden />, danger: true, run: () => setAsked({ what: 'delete', host }) } : null;
    // What is idle on the host, stopped to make room: the list is read first, and nothing stops before it is confirmed.
    const recycle: Act | null = manage ? { id: 'recycle', label: t('change.menu.recycle'), icon: <Recycle aria-hidden />, run: () => openChange({ kind: 'host', id: host.id, name: host.name }, { kind: 'recycle' }) } : null;
    const declare: Act | null = manage && !host.machine.online && host.lost_at === undefined ? { id: 'declare-lost', label: t('loss.declareMenu'), icon: <TriangleAlert aria-hidden />, danger: true, run: () => setAsked({ what: 'lost', host }) } : null;
    const moveAll: Act | null = manage ? { id: 'move-all', label: t(host.lost_at === undefined ? 'loss.moveAll' : 'loss.recover'), icon: <ArrowRightLeft aria-hidden />, run: () => openChange({ kind: 'host', id: host.id }, { kind: 'recover' }) } : null;
    return {
      primary: updateNow ?? resume ?? open,
      edit,
      entries: entriesOf([open], [updateNow, resume, pause], [edit, rename], [recycle, moveAll], [share, copy], [declare, del], [cleanup.act({kind:'host',id:host.id}, manage)]),
      keys: keysOf(rename),
      busy: busy(host.id),
    };
  };

  const bulk: BulkAct<Host>[] = [
    {
      id: 'resume',
      label: t('resources.act.resume'),
      icon: <CirclePlay aria-hidden />,
      can: (host) => holds(host.access, 'host.manage') && !host.schedulable,
      run: (host) => setSchedulable(host, true),
      done: (count) => t('hosts.bulk.resumed', { count }),
    },
    {
      id: 'pause',
      label: t('resources.act.pause'),
      icon: <CirclePause aria-hidden />,
      can: (host) => holds(host.access, 'host.manage') && host.schedulable,
      run: (host) => setSchedulable(host, false),
      done: (count) => t('hosts.bulk.paused', { count }),
    },
  ];

  const dialogs = (
    <>
      {cleanup.dialogs}
      {asked?.what === 'lost' && <DeclareLostDialog target={{ type: 'host', id: asked.host.id }} name={asked.host.name} onClose={() => setAsked(null)} />}
      {asked?.what === 'rename' && (
        <EditHost key={asked.host.id} host={asked.host} onClose={() => setAsked(null)} />
      )}
      {asked?.what === 'delete' && <RemoveHostDialog host={asked.host} onClose={() => setAsked(null)} {...(options.afterDelete === undefined ? {} : { afterDelete: options.afterDelete })} />}
    </>
  );

  return {
    list: { name: (host) => host.name, menuName: (host) => host.name, of: (host) => actsOf(host, 'row'), bulk, dialogs },
    ask: (what, host) => { if (what === 'edit') editSettings(host); else setAsked({ what, host }); },
    page: (host) => {
      const acts = actsOf(host, 'page');
      return { edit: acts.edit, primary: acts.primary, menu: acts.entries };
    },
    dialogs,
  };
}
