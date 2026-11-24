/** Member picker for a direct or group chat.
 *
 * "New message" opens a picker of the org's people. Choosing one opens the direct chat with them (the
 * one that exists already, when it does); choosing several makes a group, with a name if one is given.
 * The same picker adds people to a group, where a person from outside the org may be named too and
 * joins as a guest, who sees that chat and nothing else of the org.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Check } from 'lucide-react';
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
import { ErrorState } from '@/ui/ErrorState';

import { createChat, GROUP_NAME_MAX_CHARS } from '@/api/chats';
import { personPickerQuery } from '@/api/access';
import { usePickerCursorRestart } from '@/lib/pickerPages';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';
import { ApiError } from '@/api/client';
import type { Chat } from '@/api/chat-contract';
import type { UserRef } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { useDebounced } from '@/lib/debounce';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { useErrorText } from '@/ui/errorText';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';

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
  const [query, setQuery] = useState(''), [open, setOpen] = useState(false);
  const settled = useDebounced(query.trim()), waiting = query.trim() !== settled;
  const source = personPickerQuery(settled, orgId, guests);
  const people = useInfiniteQuery({ ...source, enabled: open && !waiting });
  usePickerCursorRestart(people, source.queryKey);
  const pages = open && !waiting ? people.data?.pages ?? [] : [];
  const chosenIds = new Set(chosen.map((user) => user.id));
  const hits = pages.flatMap((page) => page.items).filter((user) => user.kind === 'human' && user.id !== me && !exclude.has(user.id) && !chosenIds.has(user.id));
  const members = new Set(pages.flatMap((page) => page.org_member_ids ?? []));
  return <div className="flex min-w-0 flex-col gap-2">
    {chosen.length > 0 && <div className="flex max-h-[20dvh] flex-wrap gap-1 overflow-y-auto" aria-label={t('chat.people.found')}>
      {chosen.map((user) => <button key={user.id} type="button" aria-label={user.display_name} aria-pressed disabled={disabled}
        className="focus-inset flex min-w-0 items-center gap-1 rounded-md bg-accent-subtle px-2 py-1 text-sm" onClick={() => onToggle(user)}>
        <UserChip user={user} size={24} card={false} /><Check aria-hidden size={16} className="shrink-0 text-accent-text" />
      </button>)}
    </div>}
    <ResourcePickerLookup pickerKey="chat-people" label={t('chat.people.search')} query={query} onQueryChange={setQuery} onOpenChange={setOpen}
      disabled={disabled} groupHomes={false} filterLocal={false} placeholder={t('chat.people.search')} displayValue=""
      loading={waiting || people.isFetching && !people.isFetchingNextPage} empty={t('chat.people.none')}
      groups={[{ id: 'chat-people', label: t('chat.people.found'), options: hits.map((user) => ({ value: user.id, text: user.display_name,
        resource: { kind: 'user', id: user.id }, label: <UserChip user={user} size={24} card={false} />,
        description: <span className="flex gap-2"><span>@{user.username}</span>{guests && !members.has(user.id) && <span>{t('chat.people.guestChip')}</span>}</span>,
      })) }]}
      footer={people.isError ? <ErrorState compact error={people.error} onRetry={() => { void (people.isFetchNextPageError ? people.fetchNextPage() : people.refetch()); }} />
        : people.hasNextPage ? <Button size="sm" variant="ghost" pending={people.isFetchingNextPage} onClick={() => { void people.fetchNextPage(); }}>{t('ui.pagination.more')}</Button> : undefined}
      onSelect={(option) => { const user = hits.find((one) => one.id === option.value); if (user !== undefined) { setOpen(false); onToggle(user); setQuery(''); } }} />
    {guests && <HelpHint term={t('chat.people.guestChip')} what={t('chat.people.guestHint')} />}
  </div>;
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
