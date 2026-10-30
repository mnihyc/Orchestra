/** Add people, guests included, rename and leave.
 *
 * The menu of a group's header: who is in it (guests marked), adding people (a person from outside the
 * org joins as a guest, where the reader may bring one in), giving the group a name or taking it back,
 * and leaving. Whoever made the group may take others out. A direct chat has none of this: it is
 * always two people.
 */
import { EllipsisVertical, LogOut, Pencil, UserPlus, Users } from 'lucide-react';
import { UserChip } from '@/ui/UserChip';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime, formatExact } from '@/lib/time';
import { Tooltip } from '@/ui/Tooltip';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { addMember, GROUP_NAME_MAX_CHARS, removeMember, renameChat } from '@/api/chats';
import { ApiError } from '@/api/client';
import { failed } from '@/features/chat/acts';
import { useChatExportEntries } from '@/features/export/ChatMenu';
import { PeoplePicker } from '@/features/chat/NewChat';
import type { Chat, UserRef } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { chatWriter } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { refreshChat } from '@/realtime/stores/chat';

type Which = 'people' | 'add' | 'rename' | 'leave' | null;

export function GroupMenu({ chat, me, onLeft }: { chat: Chat; me: string | null; onLeft: () => void }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const describe = useErrorText();
  const [which, setWhich] = useState<Which>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [name, setName] = useState(chat.name ?? '');
  const [chosen, setChosen] = useState<UserRef[]>([]);
  const members = chat.members ?? [];
  const inside = members.some((member) => member.id === me);
  const mayManage = chat.can.send && inside;
  const made = chat.created_by?.id === me;
  const exporting = useChatExportEntries(chat, me);

  const close = () => {
    setWhich(null);
    setError(null);
    setChosen([]);
  };

  const run = async (act: () => Promise<unknown>, done: () => void) => {
    setPending(true);
    setError(null);
    try {
      await act();
      done();
      refreshChat(chat.id);
    } catch (cause) {
      setError(cause);
    } finally {
      setPending(false);
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
          ...(mayManage
            ? [
                { id: 'add', label: t('chat.group.add'), icon: <UserPlus aria-hidden />, onSelect: () => setWhich('add') },
                { id: 'rename', label: t('chat.group.rename'), icon: <Pencil aria-hidden />, onSelect: () => { setName(chat.name ?? ''); setWhich('rename'); } },
              ]
            : []),
          ...exporting,
          ...(inside
            ? [
                { type: 'separator' as const, id: 'before-leave' },
                { id: 'leave', label: t('chat.group.leave'), icon: <LogOut aria-hidden />, danger: true, onSelect: () => setWhich('leave') },
              ]
            : []),
        ]}
        trigger={<IconButton size="sm" label={t('chat.group.menu')} icon={<EllipsisVertical aria-hidden />} />}
      />

      <Dialog open={which === 'people'} onOpenChange={(open) => !open && close()} title={t('chat.group.people')} description={t('chat.group.peopleHint', { count: members.length })}>
        <ul className="flex flex-col gap-1">
          {members.map((member) => {
            const guest = chat.guests.includes(member.id);
            const expires = chat.membership_expiries?.[member.id];
            return (
              <li key={member.id} className="flex items-center gap-2 py-1">
                <span className="min-w-0 flex-1"><UserChip user={member} size={28} />
                  {expires!==undefined&&<span className="flex items-center gap-1 text-sm text-text-muted">{t('chat.people.until')} <Tooltip content={formatExact(expires,format)}><time dateTime={expires}>{formatDateTime(expires,format)}</time></Tooltip></span>}
                </span>
                {guest && <span className="shrink-0 rounded-sm bg-bg-muted px-1 text-xs text-text-muted">{t('chat.people.guestChip')}</span>}
                {made && member.id !== me && (
                  <Button
                    size="sm"
                    variant="ghost"
                    pending={pending}
                    onClick={() =>
                      void run(
                        () => removeMember(chat.id, member.id),
                        () => undefined,
                      )
                    }
                  >
                    {t('chat.group.remove')}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
        {error !== null && (
          <InlineAlert tone="danger" className="mt-3">
            {describe(error).message}
          </InlineAlert>
        )}
      </Dialog>

      <Dialog
        open={which === 'add'}
        onOpenChange={(open) => !open && !pending && close()}
        title={t('chat.group.addTitle')}
        description={t('chat.group.addHint')}
        footer={
          <>
            <Button variant="ghost" disabled={pending} onClick={close}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              pending={pending}
              blocked={chosen.length === 0}
              onClick={() =>
                void run(
                  async () => {
                    for (const user of chosen) await addMember(chat.id, user.id);
                  },
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
          onToggle={(user) => setChosen((before) => (before.some((one) => one.id === user.id) ? before.filter((one) => one.id !== user.id) : [...before, user]))}
          exclude={new Set(members.map((member) => member.id))}
          guests={guestsAllowed}
          me={me}
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
        description={t('chat.group.renameHint')}
        footer={
          <>
            <Button variant="ghost" disabled={pending} onClick={close}>
              {t('common.cancel')}
            </Button>
            <Button
              variant="primary"
              pending={pending}
              onClick={() =>
                void run(
                  async () => {
                    const next = await renameChat(chat.id, name.trim() === '' ? null : name.trim());
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
        <Input label={t('chat.new.name')} value={name} maxLength={GROUP_NAME_MAX_CHARS} onChange={(event) => setName(event.target.value)} disabled={pending} />
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
