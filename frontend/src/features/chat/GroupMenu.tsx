import { groupHomeMoveEntries } from '@/features/duplicate/move';
import { duplicateEntries } from '@/features/duplicate/target';
/** Add people, guests included, rename and leave.
 *
 * The menu of a group's header: who is in it (guests marked), adding people (a person from outside the
 * org joins as a guest, where the reader may bring one in), giving the group a name or taking it back,
 * and leaving. Current managers change its membership. A direct chat has none of this: it is
 * always two people.
 */
import { linkTo } from '@/lib/links';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { copyText } from '@/ui/clipboard';
import { EllipsisVertical, LogOut, Pencil, Share2, UserPlus, Users } from 'lucide-react';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { addMember, GROUP_NAME_MAX_CHARS, removeMember, renameChat } from '@/api/chats';
import { ApiError, client, ReceiptAcknowledged } from '@/api/client';
import { acknowledgeReceipt, failed } from '@/features/chat/acts';
import { useChatExportEntries } from '@/features/export/ChatMenu';
import { PeoplePicker } from '@/features/chat/NewChat';
import { GroupAccess } from '@/features/access/GroupAccess';
import { openAccess } from '@/features/access/AccessDialog';
import type { Chat } from '@/api/chat-contract';
import type { UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { chatWriter } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { refreshChat } from '@/realtime/stores/chat';

type Which = 'people' | 'add' | 'rename' | 'leave' | null;

type GroupMenuProps = { chat: Chat; me: string | null; onLeft: () => void };
export function GroupMenu(props: GroupMenuProps) {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <GroupMenuForReader key={`${props.chat.id}:${reader}`} {...props} />;
}

function GroupMenuForReader({ chat, me, onLeft }: GroupMenuProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const describe = useErrorText();
  const [which, setWhich] = useState<Which>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [name, setName] = useState(chat.name ?? '');
  const [chosen, setChosen] = useState<UserRef[]>([]);
  const members = chat.members ?? [];
  const inside = members.some((member) => member.id === me);
  const lastManager = me !== null && (chat.managers ?? []).includes(me) && chat.managers?.length === 1;
  const mayManage = chat.can.manage === true;
  const exporting = useChatExportEntries(chat, me);
  const nameTooLong = Array.from(name.trim()).length > GROUP_NAME_MAX_CHARS;

  const close = () => {
    setWhich(null);
    setError(null);
    setChosen([]);
  };

  const run = async (act: (reader: ReturnType<typeof libraryReader>) => Promise<unknown>, done: () => void) => {
    if (pending) return;
    const reader = libraryReader();
    setPending(true);
    setError(null);
    try {
      await act(reader);
      if (!sameLibraryReader(reader)) return;
      done();
      refreshChat(chat.id);
    } catch (cause) {
      if (!sameLibraryReader(reader)) return;
      if (acknowledgeReceipt(cause)) {
        done();
        refreshChat(chat.id);
      } else setError(cause);
    } finally {
      if (sameLibraryReader(reader)) setPending(false);
    }
  };

  const addChosen = async (reader: ReturnType<typeof libraryReader>) => {
    let acknowledged: ReceiptAcknowledged | undefined;
    let changed = false;
    let complete = false;
    try {
      for (const user of chosen) {
        if (!sameLibraryReader(reader)) return;
        try { await addMember(chat.id, user.id); }
        catch (cause) {
          if (!(cause instanceof ReceiptAcknowledged)) throw cause;
          acknowledged ??= cause;
        }
        if (!sameLibraryReader(reader)) return;
        changed = true;
        // A later refusal leaves only failed/unattempted people selected. A
        // committed addition is never repeated just to recover a readable row.
        setChosen((before) => before.filter((one) => one.id !== user.id));
      }
      complete = true;
    } finally {
      if (sameLibraryReader(reader)) {
        if (acknowledged !== undefined) acknowledgeReceipt(acknowledged);
        if (changed && !complete) refreshChat(chat.id);
      }
    }
  };

  const guestsAllowed = true;
  return (
    <>
      <Menu
        label={t('chat.group.menu')}
        align="end"
        entries={[
          { id: 'people', label: t('chat.group.people'), icon: <Users aria-hidden />, onSelect: () => setWhich('people') },
          { id: 'share', label: t('everything.share'), icon: <Share2 aria-hidden />, onSelect: () => openAccess({ kind: 'chat', id: chat.id, name: chat.name ?? t('access.kinds.chat') }) },
          ...(mayManage
            ? [
                { id: 'add', label: t('chat.group.add'), icon: <UserPlus aria-hidden />, onSelect: () => setWhich('add') },
                { id: 'rename', label: t('chat.group.rename'), icon: <Pencil aria-hidden />, onSelect: () => { setName(chat.name ?? ''); setWhich('rename'); } },
              ]
            : []),
          { id: 'copy-link', action: 'copyLink', label: t('actions.copyLink.label'), onSelect: () => void copyText(linkTo({ kind: 'chat', id: chat.id }), t('common.linkCopied')) },
          ...groupHomeMoveEntries(t, chat),
          ...exporting,
          ...duplicateEntries(t, { kind: 'chat', id: chat.id, name: chat.name ?? t('chat.title.groupUnnamed') }),
          ...(inside
            ? ([
                { type: 'separator' as const, id: 'before-leave' },
                { id: 'leave', label: t('chat.group.leave'), icon: <LogOut aria-hidden />, danger: true, disabled: lastManager ? t('chat.group.keepManager') : false, onSelect: () => setWhich('leave') },
              ] satisfies MenuEntry[])
            : []),
        ]}
        trigger={<IconButton size="sm" label={t('chat.group.menu')} icon={<EllipsisVertical aria-hidden />} />}
      />

      <Dialog open={which === 'people'} onOpenChange={(open) => !open && close()} title={t('chat.group.people')}>
        <GroupAccess key={chat.id} chat={chat} membersHeading={false} />
      </Dialog>

      <Dialog
        open={which === 'add'}
        onOpenChange={(open) => !open && !pending && close()}
        title={t('chat.group.addTitle')}
        footer={
          <>
            <Button variant="ghost" disabled={pending} onClick={close}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              pending={pending}
              blocked={chosen.length === 0 || !mayManage}
              onClick={() =>
                void run(
                  addChosen,
                  close,
                )
              }
            >
              {chosen.length === 0 ? t('chat.group.addTitle') : t('chat.group.addButton', { count: chosen.length })}
            </Button>
          </>
        }
      >
        <PeoplePicker
          orgId={chat.org_id}
          chosen={chosen}
          onToggle={(user) => { if (!pending) setChosen((before) => (before.some((one) => one.id === user.id) ? before.filter((one) => one.id !== user.id) : [...before, user])); }}
          exclude={new Set(members.map((member) => member.id))}
          guests={guestsAllowed}
          me={me}
          disabled={pending}
        />
        {error !== null && (
          <InlineAlert tone="danger" className="mt-3" title={t('chat.group.addFailed')}>
            {error instanceof ApiError && error.is('sharing.external_forbidden') ? t('chat.new.external') : describe(error).message}
          </InlineAlert>
        )}
      </Dialog>

      <Dialog
        open={which === 'rename'}
        onOpenChange={(open) => !open && !pending && close()}
        title={t('chat.group.renameTitle')}
        footer={
          <>
            <Button variant="ghost" disabled={pending} onClick={close}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              pending={pending}
              blocked={nameTooLong || !mayManage}
              onClick={() =>
                void run(
                  async (reader) => {
                    const next = await renameChat(chat.id, name.trim() === '' ? null : name.trim());
                    if (!sameLibraryReader(reader)) return;
                    chatWriter.apply(chat.id, { chat: next }, next.seq);
                  },
                  close,
                )
              }
            >
              {t('chat.group.renameButton')}
            </Button>
          </>
        }
      >
        <Input label={t('chat.new.name')} value={name} onChange={(event) => setName(event.target.value)} disabled={pending} error={nameTooLong ? t('chat.new.nameTooLong', { number: formatNumber(GROUP_NAME_MAX_CHARS, locale) }) : undefined} />
        {error !== null && (
          <InlineAlert tone="danger" className="mt-3">
            {describe(error).message}
          </InlineAlert>
        )}
      </Dialog>

      <ConfirmDialog
        open={which === 'leave'}
        onOpenChange={(open) => !open && close()}
        title={t('chat.group.leaveTitle')}
        confirmLabel={t('chat.group.leave')}
        danger
        pending={pending}
        error={error}
        onConfirm={() =>
          void run(
            async () => {
              if (me !== null) await removeMember(chat.id, me);
            },
            () => {
              close();
              onLeft();
            },
          ).catch((cause: unknown) => failed(i18n.t('chat.failed.leave'), cause))
        }
      >
        {t('chat.group.leaveBody')}
      </ConfirmDialog>
    </>
  );
}
