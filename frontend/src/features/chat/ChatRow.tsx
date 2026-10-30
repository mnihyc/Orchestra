/** One row of a list of chats: avatars, name, the first line of the last message, its time, the unread dot and the mentions, and the row's menu.
 *
 * Rows the reader only reads (the chats of other people, which they read as an admin) are drawn
 * without a bell or a menu. The list response carries each preview, so drawing rows needs no
 * extra message requests.
 */
import { useMutation } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { BellOff, EyeOff, MessagesSquare } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { setChatState } from '@/api/chats';
import { failed } from '@/features/chat/acts';
import { chatTitle } from '@/features/chat/ChatView';
import { PersonThingProvider } from '@/features/people/personContext';
import type { MessagesSearch } from '@/features/chat/Messages';
import { plain } from '@/features/chat/text';
import type { Chat } from '@/generated/api-types';
import { statesWriter, useChatUnread } from '@/realtime/stores/chat';
import { Avatar } from '@/ui/Avatar';
import { AvatarStack } from '@/ui/AvatarStack';
import { Badge } from '@/ui/Badge';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { ActionIcon } from '@/ui/actions';
import { RelativeTime } from '@/ui/RelativeTime';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { UserChip } from '@/ui/UserChip';

/** How many characters of the last message a row shows. */
const PREVIEW_CHARS = 80;

/** The first line already carried by the list, with the group author's name. */
export function previewText(chat: Chat, deleted: string, whole = false): string {
  const message = chat.preview;
  if (message === null) return '';
  const who = message.author !== null && 'display_name' in message.author ? message.author.display_name : '';
  const line = whole ? message.text : (message.text.split('\n').find((one) => one.trim() !== '') ?? '');
  const text = message.deleted ? deleted : whole ? plain(line) : Array.from(plain(line)).slice(0, PREVIEW_CHARS).join('');
  return chat.kind === 'group' && who !== '' ? `${who}: ${text}` : text;
}

function rowAvatars(chat: Chat, me: string | null) {
  const others = (chat.members ?? []).filter((member) => member.id !== me);
  if (others.length > 0) return <PersonThingProvider thing={{ kind: 'chat', id: chat.id }}><AvatarStack users={others} max={2} size={24} /></PersonThingProvider>;
  return <Avatar text="#" color={0} kind="org" size={32} />;
}

export function ChatRow({
  chat,
  slug,
  selected,
  me,
  search,
  quiet = false,
}: {
  chat: Chat;
  slug: string;
  selected: boolean;
  me: string | null;
  search: MessagesSearch;
  /** Rows of chats the reader only reads are drawn without a bell or a menu. */
  quiet?: boolean;
}) {
  const { t } = useTranslation();
  const preview = previewText(chat, t('chat.deleted'));
  const directPerson = chat.kind === 'direct' ? chat.members?.find((member) => member.id !== me) : undefined;
  const previewPerson = chat.kind === 'group' && chat.preview?.author !== null && chat.preview?.author !== undefined && 'id' in chat.preview.author ? chat.preview.author : undefined;
  const previewPrefix = previewPerson === undefined ? '' : `${previewPerson.display_name}: `;
  const previewBody = previewPrefix !== '' && preview.startsWith(previewPrefix) ? preview.slice(previewPrefix.length) : preview;
  const { unread, mentions, state } = useChatUnread(chat.id, chat.last_message_seq, chat.my);
  const name = chat.kind === 'node' ? (chat.node?.name ?? '') : chatTitle(chat, me, t('chat.title.groupUnnamed'));
  const muted = state?.level === 'off';

  const change = useMutation({
    mutationFn: (next: { level?: 'off' | null; hidden?: boolean }) => setChatState(chat.id, next),
    onSuccess: (next) => {
      statesWriter.put(next);
    },
    onError: (error) => failed(t('chat.failed.bell'), error),
  });

  const entries: MenuEntry[] = quiet
    ? []
    : [
        {
          id: 'mute',
          action: muted ? 'unmute' : 'mute', label: muted ? t('chat.row.unmute') : t('chat.row.mute'),
          icon: <BellOff aria-hidden />,
          onSelect: () => change.mutate({ level: muted ? null : 'off' }),
        },
        { id: 'hide', label: t('chat.row.hide'), icon: <EyeOff aria-hidden />, onSelect: () => change.mutate({ hidden: true }) },
      ];

  return (
    <ContextMenu label={t('actions.actions.named', { name })} entries={entries} disabled={quiet}>
      <li className={cx('group relative flex items-center gap-3 rounded-md px-3 py-2 hover:bg-bg-subtle', selected && 'bg-bg-muted hover:bg-bg-muted')}>
        {directPerson === undefined && <span className={cx('flex w-8 shrink-0 justify-center', chat.kind !== 'node' && 'relative z-10')}>{chat.kind === 'node' ? <MessagesSquare size={16} aria-hidden className="text-text-muted" /> : rowAvatars(chat, me)}</span>}
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex items-baseline gap-2">
            {directPerson !== undefined && <span className="relative z-10 min-w-0"><UserChip user={directPerson} size={32} thing={{ kind: 'chat', id: chat.id }} nameClassName={unread ? 'font-semibold' : ''} /></span>}
            <Link
              to="/org/$slug/messages"
              params={{ slug }}
              search={{ ...search, chat: chat.id, message: undefined }}
              aria-current={selected ? 'true' : undefined}
              aria-label={directPerson === undefined ? undefined : t('chat.row.openWith', { name })}
              title={directPerson === undefined ? undefined : t('chat.row.openWith', { name })}
              className={cx('min-w-0 flex-1 truncate outline-none after:absolute after:inset-0 focus-visible:after:outline-2 focus-visible:after:outline-focus', directPerson === undefined && (unread ? 'font-semibold text-text' : 'font-medium text-text'))}
            >
              {directPerson === undefined ? name : <MessagesSquare size={16} aria-hidden className="text-text-muted" />}
            </Link>
            <AsAdminMark of={adminAccessOf(chat)?.place} responsive />
            {chat.last_message_at !== null && <RelativeTime at={chat.last_message_at} className="shrink-0 text-xs text-text-muted" />}
          </div>
          <div className={cx('flex items-center gap-2', directPerson !== undefined && 'pl-10')}>
            {previewPerson !== undefined && chat.preview !== null && <span className="relative z-10 min-w-0 max-w-1/2 truncate"><UserChip user={previewPerson} size={24} thing={{ kind: 'chat', id: chat.id, anchor: { message: chat.preview.id } }} />:</span>}
            <span data-overflow-text={previewText(chat, t('chat.deleted'), true)} className={cx('min-w-0 flex-1 truncate text-sm', unread ? 'text-text' : 'text-text-muted')}>{previewBody}</span>
            {mentions > 0 ? (
              <Badge tone="accent" count={mentions} label={t('chat.unread.mentions', { count: mentions })} />
            ) : (
              unread && (
                <>
                  <span aria-hidden className="size-2 shrink-0 rounded-full bg-accent-solid" />
                  <span className="sr-only">{t('chat.unread.dot')}</span>
                </>
              )
            )}
            {muted && <BellOff size={16} aria-label={t('chat.row.muted')} className="shrink-0 text-text-subtle" />}
          </div>
        </div>
        {!quiet && (
          <span className="relative z-10 opacity-0 group-hover:opacity-100 group-focus-within:opacity-100">
            <Menu
              label={t('actions.actions.named', { name })}
              align="end"
              entries={entries}
              trigger={<IconButton size="sm" label={t('actions.actions.named', { name })} icon={<ActionIcon action="actions" />} />}
            />
          </span>
        )}
      </li>
    </ContextMenu>
  );
}
