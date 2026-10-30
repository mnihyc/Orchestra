/** Who is typing.
 *
 * One quiet line under the messages: up to three names, then "Several people are typing…". Each
 * person is shown for six seconds after the last sign of it, which is the protocol's own measure.
 * The live region stays mounted without taking conversation space while nobody is typing.
 */
import { Trans, useTranslation } from 'react-i18next';

import { PersonThingProvider } from '@/features/people/personContext';
import { type TypingMark, useTyping } from '@/realtime/stores/chat';
import { AvatarStack } from '@/ui/AvatarStack';
import { UserChip } from '@/ui/UserChip';
import { cx } from '@/ui/cx';

/** The most names the line says before it says "several". */
const NAMED_MOST = 3;

export function Typing({ typing, me, chatId }: { typing: ReadonlyMap<string, TypingMark>; me: string | null; chatId: string }) {
  const { t } = useTranslation();
  const users = useTyping(typing, me);
  const names = users.map((user) => user.display_name);
  const text =
    users.length === 0
      ? ''
      : users.length > NAMED_MOST
        ? t('chat.typing.several')
        : users.length === 1
          ? t('chat.typing.one', { name: names[0] })
          : users.length === 2
            ? t('chat.typing.two', { first: names[0], second: names[1] })
            : t('chat.typing.three', { first: names[0], second: names[1], third: names[2] });
  return (
    <PersonThingProvider thing={{ kind: 'chat', id: chatId }}><div className={cx('shrink-0 text-xs text-text-muted', users.length === 0 ? 'sr-only' : 'flex min-h-(--control-sm) items-center gap-1 px-3')}>
      <span role="status" aria-live="polite" className="sr-only">{text}</span>
      {users.length === 1 && users[0] !== undefined && <Trans i18nKey="chat.typing.oneChip" components={{ person: <UserChip user={users[0]} size={24} /> }} />}
      {users.length > 1 && <><AvatarStack users={users} max={3} size={24} /><span>{t('chat.typing.plural')}</span></>}
    </div></PersonThingProvider>
  );
}
