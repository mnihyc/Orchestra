/** A group's Share surface is its native roster and visibility, inside the existing Access dialog. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { useFormatContext } from '@/lib/appearance';
import { formatDateTime, formatExact } from '@/lib/time';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { linkTo } from '@/lib/links';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import { UserMinus } from 'lucide-react';
import { addMember, chatKey, chatQuery, orgChatsKey, removeMember, setGroupManager, updateGroup } from '@/api/chats';
import { client, ReceiptAcknowledged } from '@/api/client';
import { acknowledgeReceipt } from '@/features/chat/acts';
import { PeoplePicker } from '@/features/chat/NewChat';
import { KnownNodeChip } from '@/features/people/NamedChip';
import { refreshChat, chatWriter } from '@/realtime/stores/chat';
import type { Chat } from '@/api/chat-contract';
import type { UserRef } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { AdminReachNotice } from './AdminReachNotice';

/** The same compact audience fact is used in the dialog and the chat header. */
export function GroupAccessSummary({ chat, me }: { chat: Chat; me: string | null }) {
  const { t } = useTranslation();
  const manager = me !== null && (chat.managers ?? []).includes(me);
  const member = me !== null && (chat.members ?? []).some((user) => user.id === me);
  return <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
    {manager ? <span>{t('chat.group.manager')}</span> : member && <span>{t('chat.group.member')}</span>}
    {chat.visibility === 'public' ? <><span>{t('chat.group.publicIn')}</span>{chat.home !== null && chat.home !== undefined && <KnownNodeChip node={chat.home} />}</>
      : <><span>{t('chat.group.private')}</span>{chat.home !== null && chat.home !== undefined && <KnownNodeChip node={chat.home} />}</>}
  </div>;
}

export function ChatAccessPanel({ id, fallback }: { id: string; fallback: ReactNode }) {
  const query = useQuery(chatQuery(id));
  if (query.isError) return <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  if (query.data === undefined) return <SkeletonRows rows={5} />;
  return query.data.kind === 'group' ? <GroupAccess key={query.data.id} chat={query.data} /> : fallback;
}

type GroupAccessProps = { chat: Chat; membersHeading?: boolean };
export function GroupAccess(props: GroupAccessProps) {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <GroupAccessForReader key={`${props.chat.id}:${reader}`} {...props} />;
}

function GroupAccessForReader({ chat, membersHeading = true }: GroupAccessProps) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const format = useFormatContext();
  const me = useStore(client.session, (session) => session.userId);
  const [adding, setAdding] = useState(false);
  const [moving, setMoving] = useState(false);
  const [home, setHome] = useState<PickedPlace | null>(null);
  const [chosen, setChosen] = useState<UserRef[]>([]);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const members = chat.members ?? [];
  const managers = new Set(chat.managers ?? []);
  const manage = chat.can.manage === true;
  const manageReason = manage ? undefined : t('chat.group.managerNeeded');
  const refresh = () => { refreshChat(chat.id); void cache.invalidateQueries({ queryKey: chatKey(chat.id) }); void cache.invalidateQueries({ queryKey: orgChatsKey(chat.org_id) }); };
  const accept = (next: Chat) => {
    cache.setQueryData(chatKey(chat.id), next);
    chatWriter.apply(chat.id, { chat: next }, next.seq);
  };
  const run = async (act: () => Promise<Chat> | Promise<void>, accepted?: () => void) => {
    if (pending) return;
    const reader = libraryReader();
    setPending(true); setError(null);
    try { const next = await act(); if (!sameLibraryReader(reader)) return; if (next !== undefined) accept(next); refresh(); accepted?.(); }
    catch (cause) { if (!sameLibraryReader(reader)) return; if (acknowledgeReceipt(cause)) { refresh(); accepted?.(); } else setError(cause); }
    finally { if (sameLibraryReader(reader)) setPending(false); }
  };
  const add = async () => {
    const reader = libraryReader();
    let changed = false;
    let acknowledged: ReceiptAcknowledged | undefined;
    try {
      for (const user of chosen) {
        if (!sameLibraryReader(reader)) return;
        try { const next = await addMember(chat.id, user.id); if (!sameLibraryReader(reader)) return; accept(next); }
        catch (cause) { if (!(cause instanceof ReceiptAcknowledged)) throw cause; acknowledged ??= cause; }
        if (!sameLibraryReader(reader)) return;
        changed = true;
        setChosen((before) => before.filter((held) => held.id !== user.id));
      }
      setAdding(false);
    } finally { if (sameLibraryReader(reader)) { if (acknowledged !== undefined) acknowledgeReceipt(acknowledged); if (changed) refresh(); } }
  };
  return <div className="flex min-w-0 flex-col gap-4">
    <AdminReachNotice of={chat.can.admin_access?.place} private object={{ kind: 'chat', id: chat.id }} />
    <div className="flex flex-wrap items-center justify-between gap-2">
      <GroupAccessSummary chat={chat} me={me} />
      <CopyButton value={linkTo({ kind: 'chat', id: chat.id })} label={t('places.menu.copyLink')} />
    </div>
    <Tooltip content={manageReason}>
      <span tabIndex={manage ? undefined : 0} className="block">
        <Select<'private' | 'public'> label={t('chat.group.visibility')} value={chat.visibility} disabled={!manage || pending}
          options={[{ value: 'private', label: t('chat.group.private') }, { value: 'public', label: t('chat.group.public') }]}
          onValueChange={(visibility) => void run(() => updateGroup(chat.id, { visibility }))} />
      </span>
    </Tooltip>
    <div className="flex min-w-0 flex-col gap-2">
      <div className="flex min-w-0 flex-wrap items-center gap-2"><span className="text-sm text-text-muted">{t('chat.group.home')}</span>
        {chat.home === null || chat.home === undefined ? <span>–</span> : <KnownNodeChip node={chat.home} />}
        <Button size="sm" variant="ghost" blocked={!manage} disabled={pending} title={manageReason} onClick={() => setMoving((value) => !value)}>{t('chat.group.moveHome')}</Button>
      </div>
      {moving && manage && <><PlacePicker label={t('chat.group.home')} value={home} onChange={setHome} rootOrgId={chat.org_id} includeThreads disabled={pending} />
        <div className="flex justify-end gap-2"><Button size="sm" variant="ghost" disabled={pending} onClick={() => setMoving(false)}>{t('common.cancel')}</Button>
          <Button size="sm" pending={pending} disabled={home === null} onClick={() => { if (home !== null) void run(() => updateGroup(chat.id, { home_id: home.id }), () => { setMoving(false); setHome(null); }); }}>{t('chat.group.move')}</Button></div></>}
    </div>
    <section className="min-w-0">
      <div className="mb-2 flex items-center justify-between gap-2">
        <h3 className={membersHeading ? "text-sm font-medium" : "sr-only"}>{t('chat.group.people')}</h3>
        <Tooltip content={manageReason}><Button size="sm" variant="ghost" blocked={!manage} disabled={pending} onClick={() => setAdding(!adding)}>{t('chat.group.add')}</Button></Tooltip>
      </div>
      {members.length === 0 ? <p className="text-sm text-text-muted">{chat.members === null ? '–' : t('chat.group.noMembers')}</p> : <Table label={t('chat.group.people')} rows={members} rowKey={(user) => user.id}
        columns={[
          { id: 'name', header: t('chat.group.person'), grow: true, cell: (user) => {
            const expires = chat.membership_expiries?.[user.id];
            return <div className="flex min-w-0 flex-col gap-1"><UserChip user={user} />
              {(chat.guests.includes(user.id) || expires !== undefined) && <div className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
                {chat.guests.includes(user.id) && <span>{t('chat.people.guestChip')}</span>}
                {expires !== undefined && <span>{t('chat.people.until')} <Tooltip content={formatExact(expires, format)}><time dateTime={expires}>{formatDateTime(expires, format)}</time></Tooltip></span>}
              </div>}
            </div>;
          } },
          { id: 'role', header: t('chat.group.role'), cell: (user) => {
            const manager = managers.has(user.id);
            const last = manager && managers.size === 1;
            const reason = last ? t('chat.group.keepManager') : manageReason;
            return <Tooltip content={reason}><span tabIndex={reason === undefined ? undefined : 0}>
              <Select size="sm" hideLabel label={t('chat.group.memberRole', { name: user.display_name })} value={manager ? 'manager' : 'member'} disabled={!manage || last || pending}
                options={[{ value: 'member', label: t('chat.group.member') }, { value: 'manager', label: t('chat.group.manager') }]}
                onValueChange={(role) => void run(() => setGroupManager(chat.id, user.id, role === 'manager'))} />
            </span></Tooltip>;
          } },
          { id: 'remove', header: t('chat.group.remove'), hideHeader: true, align: 'end', cell: (user) => <IconButton size="sm" icon={<UserMinus aria-hidden />} label={t('chat.group.removePerson', { name: user.display_name })}
            pending={pending} blocked={!manage || managers.has(user.id) && managers.size === 1} reason={managers.has(user.id) && managers.size === 1 ? t('chat.group.keepManager') : manageReason}
            onClick={() => void run(() => removeMember(chat.id, user.id))} /> },
        ]} />}
    </section>
    {adding && <div className="flex min-w-0 flex-col gap-2">
      <PeoplePicker orgId={chat.org_id} chosen={chosen} onToggle={(user) => { if (!pending) setChosen((before) => before.some((held) => held.id === user.id) ? before.filter((held) => held.id !== user.id) : [...before, user]); }} exclude={new Set(members.map((user) => user.id))} guests me={me} disabled={pending} />
      <div className="flex justify-end gap-2">
        <Button size="sm" variant="ghost" disabled={pending} onClick={() => setAdding(false)}>{t('common.cancel')}</Button>
        <Tooltip content={manageReason ?? (chosen.length === 0 ? t('chat.group.choosePeople') : undefined)}><Button size="sm" variant="primary" pending={pending} blocked={chosen.length === 0 || !manage} onClick={() => void run(add)}>{t('common.add')}</Button></Tooltip>
      </div>
    </div>}
    {error !== null && <ErrorAlert error={error} action={<Button size="sm" onClick={refresh}>{t('common.refresh')}</Button>} />}
  </div>;
}
