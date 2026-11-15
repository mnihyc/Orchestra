/** Member picker for a direct or group chat.
 *
 * "New message" opens a picker of the org's people. Choosing one opens the direct chat with them (the
 * one that exists already, when it does); choosing several makes a group, with a name if one is given.
 * The same picker adds people to a group, where a person from outside the org may be named too and
 * joins as a guest, who sees that chat and nothing else of the org.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Check, Search } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { acknowledgeReceipt } from '@/features/chat/acts';
import { PlacePicker, type PickedPlace } from '@/features/settings/PlacePicker';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Select } from '@/ui/Select';
import { Switch } from '@/ui/Switch';
import { UserChip } from '@/ui/UserChip';
import { HelpHint } from '@/ui/HelpHint';
import { Pagination } from '@/ui/Pagination';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';

import { createChat, GROUP_NAME_MAX_CHARS, PEOPLE_SEARCH_MIN_CHARS, peopleQuery } from '@/api/chats';
import { membersQuery } from '@/api/directory';
import { ApiError } from '@/api/client';
import type { Chat } from '@/api/chat-contract';
import type { UserRef } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { useDebounced } from '@/lib/debounce';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { InlineAlert } from '@/ui/InlineAlert';
import { fieldBoxClasses, Input } from '@/ui/Input';
import { Skeleton } from '@/ui/Skeleton';

export interface PeoplePickerProps {
  orgId: string;
  chosen: readonly UserRef[];
  onToggle: (user: UserRef) => void;
  /** People already in the chat, who are not offered. */
  exclude: ReadonlySet<string>;
  /** Whether a person from outside the org may be named, by their exact username or address, to join as a guest. */
  guests: boolean;
  me: string | null;
  disabled?: boolean;
}

/** A searchable list of the org's people with those who are chosen marked. */
export function PeoplePicker({ orgId, chosen, onToggle, exclude, guests, me, disabled = false }: PeoplePickerProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const settled = useDebounced(query).trim();
  const members = useInfiniteQuery(membersQuery(orgId, { direct: false, q: settled }));
  const outside = useQuery({ ...peopleQuery(null, settled), enabled: guests && Array.from(settled).length >= PEOPLE_SEARCH_MIN_CHARS });
  const insiders = (members.data?.pages.flatMap((page) => page.items.map((member) => member.user)) ?? []).filter((user) => user.id !== me && !exclude.has(user.id));
  const insiderIds = new Set(insiders.map((user) => user.id));
  const strangers = (outside.data ?? []).filter((user) => user.id !== me && !exclude.has(user.id) && !insiderIds.has(user.id));
  const row = (user: UserRef, guest: boolean) => {
    const on = chosen.some((one) => one.id === user.id);
    return (
      <button
        key={user.id}
        type="button"
        aria-pressed={on}
        disabled={disabled}
        onClick={() => onToggle(user)}
        className={cx('focus-inset flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-base hover:bg-bg-muted', on && 'bg-accent-subtle')}
      >
        <UserChip user={user} size={24} card={false} />
        <span className="truncate text-sm text-text-muted">@{user.username}</span>
        <span className="ml-auto flex shrink-0 items-center gap-2">
          {guest && <span className="rounded-sm bg-bg-muted px-1 text-xs text-text-muted">{t('chat.people.guestChip')}</span>}
          {/* A mark and not only a tint: a choice is not told by colour alone. */}
          {on && <Check aria-hidden size={16} className="text-accent-text" />}
        </span>
      </button>
    );
  };
  return (
    <div className="flex flex-col gap-2">
      <label className="relative block">
        <span className="sr-only">{t('chat.people.search')}</span>
        <Search aria-hidden size={16} className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-text-subtle" />
        <input
          type="search"
          disabled={disabled}
          value={query}
          autoComplete="off"
          placeholder={t('chat.people.search')}
          onChange={(event) => setQuery(event.target.value)}
          className={`${fieldBoxClasses} input-leading-icon`}
        />
      </label>
      <div role="group" aria-label={t('chat.people.found')} className="flex max-h-[30dvh] min-h-16 flex-col gap-1 overflow-y-auto rounded-lg border border-border p-1">
        {members.isError ? <ErrorState compact error={members.error} onRetry={() => void members.refetch()} /> : members.isPending ? (
          <Skeleton className="h-8 w-full" />
        ) : insiders.length === 0 && strangers.length === 0 ? (
          <EmptyState compact className="px-2" title={t('chat.people.none')} />
        ) : (
          <>
            {insiders.map((user) => row(user, false))}
            {strangers.map((user) => row(user, true))}
          </>
        )}
      </div>
      {outside.isError && <ErrorAlert error={outside.error} action={<Button size="sm" onClick={() => void outside.refetch()}>{t('common.tryAgain')}</Button>} />}
      <Pagination hasMore={members.hasNextPage} loading={members.isFetchingNextPage} error={members.error} onMore={() => void members.fetchNextPage()} />
      {guests && <HelpHint term={t('chat.people.guestChip')} what={t('chat.people.guestHint')} />}
    </div>
  );
}

export interface NewChatProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  orgId: string;
  me: string | null;
  /** The actual place that opened the dialog, within this root organization. */
  defaultHome?: PickedPlace | null;
  /** Told which chat was opened or made. */
  onOpened: (chat: Chat) => void;
}

/** A reader change retires the old person's selection and any late accepted result. */
export function NewChat(props: NewChatProps) {
  const actor = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <NewChatDialog key={`${props.orgId}:${actor}`} {...props} />;
}

/** The dialog behind New message; explicit group fields never turn a group into a direct chat. */
function NewChatDialog({ open, onOpenChange, orgId, me, onOpened, defaultHome = null }: NewChatProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const describe = useErrorText();
  const actKey = useActKey();
  const [chosen, setChosen] = useState<UserRef[]>([]);
  const [name, setName] = useState('');
  const [group, setGroup] = useState(false);
  const [home, setHome] = useState<PickedPlace | null>(defaultHome);
  const [visibility, setVisibility] = useState<'private' | 'public'>('private');
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const nameTooLong = Array.from(name.trim()).length > GROUP_NAME_MAX_CHARS;
  const toggle = (user: UserRef) => {
    setError(null);
    const next = chosen.some((one) => one.id === user.id) ? chosen.filter((one) => one.id !== user.id) : [...chosen, user];
    setChosen(next);
    if (next.length > 1) setGroup(true);
  };
  const clear = () => { setChosen([]); setName(''); setGroup(false); setHome(defaultHome); setVisibility('private'); };
  const submit = async () => {
    if (chosen.length === 0 || pending || group && nameTooLong) return;
    const reader = libraryReader();
    setPending(true); setError(null);
    const body = { user_ids: chosen.map((user) => user.id), ...(group ? {
      ...(name.trim() === '' ? {} : { name: name.trim() }),
      ...(home === null ? {} : { home_id: home.id }), visibility,
    } : {}) };
    try {
      const chat = await createChat(orgId, body, actKey(body));
      if (!sameLibraryReader(reader)) return;
      onOpened(chat); onOpenChange(false); clear();
    } catch (failed) {
      if (!sameLibraryReader(reader)) return;
      if (acknowledgeReceipt(failed)) { onOpenChange(false); clear(); }
      else setError(failed);
    } finally { if (sameLibraryReader(reader)) setPending(false); }
  };
  const title = chosen.length === 0 ? t('chat.new.button') : !group && chosen.length === 1 ? t('chat.new.direct', { name: chosen[0]?.display_name ?? '' })
    : group ? t('chat.new.group', { count: chosen.length + 1, replace: { count: formatNumber(chosen.length + 1, locale) } }) : t('chat.new.button');
  return <Dialog open={open} onOpenChange={(next) => !pending && onOpenChange(next)} title={t('chat.new.title')} dismissible={!pending}
    footer={<><Button variant="ghost" disabled={pending} onClick={() => onOpenChange(false)}>{t('common.cancel')}</Button>
      <Button action="newMessage" variant="primary" pending={pending} blocked={chosen.length === 0 || group && nameTooLong} onClick={() => void submit()}>{title}</Button></>}>
    <div className="flex min-w-0 flex-col gap-3">
      {group && <Input label={t('chat.new.name')} optional value={name} onChange={(event) => { setName(event.target.value); setError(null); }} disabled={pending}
        error={nameTooLong ? t('chat.new.nameTooLong', { number: formatNumber(GROUP_NAME_MAX_CHARS, locale) }) : undefined} />}
      <Switch label={t('chat.new.asGroup')} checked={group} disabled={pending || chosen.length > 1}
        tooltip={chosen.length > 1 ? t('chat.new.multipleGroup') : undefined} onCheckedChange={(value) => { setGroup(value); setError(null); }} />
      <PeoplePicker orgId={orgId} chosen={chosen} onToggle={toggle} exclude={new Set()} guests={false} me={me} disabled={pending} />
      {group && <>
        <PlacePicker label={t('chat.group.home')} value={home} onChange={setHome} rootOrgId={orgId} includeThreads disabled={pending} />
        {home === null && <span className="text-sm text-text-muted">{t('chat.new.rootHome')}</span>}
        <Select<'private' | 'public'> label={t('chat.group.visibility')} value={visibility} disabled={pending} onValueChange={setVisibility}
          options={[{ value: 'private', label: t('chat.group.private') }, { value: 'public', label: t('chat.group.public') }]} />
      </>}
      {error !== null && <InlineAlert tone="danger" title={t('chat.new.failed')}>{error instanceof ApiError && error.is('sharing.external_forbidden') ? t('chat.new.external') : describe(error).message}</InlineAlert>}
    </div>
  </Dialog>;
}
