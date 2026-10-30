/** Follow level: chosen, or the default once the person takes part.
 *
 * Whether the reader follows a chat and how: every message, mentions only, or nothing. The bell
 * shows that effective state, with Default named in its accessible description and open menu.
 * Nothing applies until they engage, unless the server supplies an owner's or followed-place default.
 * The members of a direct or
 * group chat follow it from the start. A pending choice keeps the control in place; only its
 * acknowledgement changes the served level, so a refusal cannot restore older unread counters.
 */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { defaultLevelName, levelOptions } from '@/features/notifications/levelWords';

import { setChatState } from '@/api/chats';
import { failed } from '@/features/chat/acts';
import { BellTrigger } from '@/features/notifications/BellTrigger';
import type { Chat, ChatLevel, ChatState } from '@/generated/api-types';
import { statesWriter } from '@/realtime/stores/chat';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { i18n } from '@/i18n';

/** What a level comes to when the person has not chosen one. */
function defaultFor(chat: Pick<Chat, 'kind'>, state: ChatState | null): ChatLevel {
  const served = state?.default_level;
  if (served !== null && served !== undefined) return served;
  return state?.engaged === true || chat.kind !== 'node' ? 'all' : 'off';
}

export function Bell({ chat, state }: { chat: Chat; state: ChatState | null }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const chosen = state?.level ?? null;
  const effective: ChatLevel = state?.effective_level ?? 'off';

  const change = useMutation({ mutationFn: (level: ChatLevel | null) => setChatState(chat.id, { level }),
    onSuccess: (next) => statesWriter.put(next),
    onError: (error) => failed(i18n.t('chat.failed.bell'), error) });
  const choose = (level: ChatLevel | null) => {
    if (change.isPending || level === chosen) return;
    setOpen(false);
    change.mutate(level);
  };

  const entries: MenuEntry[] = [
    {
      type: 'radio',
      id: 'level',
      label: t('people.chatBellHeading', { kind: chat.kind === 'node' ? t(`chat.kind.${chat.node?.kind ?? 'topic'}`) : t(chat.kind === 'group' ? 'people.groupChat' : 'people.directChat') }),
      value: chosen ?? 'default',
      onValueChange: (value) => choose(value === 'default' ? null : (value as ChatLevel)),
      options: [
        { value: 'default', name: defaultLevelName(t, defaultFor(chat, state)) },
        ...levelOptions(t, ['all', 'mentions', 'off'] as const),
      ],
    },
  ];

  return (
    <Menu
      label={t('chat.bell.label')}
      align="end"
      open={open}
      onOpenChange={(next) => { if (!change.isPending) setOpen(next); }}
      entries={entries}
      trigger={
        <BellTrigger label={t('actions.notificationSettings.label')} scopeLabel={t('notificationBell.scope.chat')} level={effective} inherited={chosen === null} pending={change.isPending} />
      }
    />
  );
}
