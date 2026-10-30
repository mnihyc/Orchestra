/** The bar of a selection across turns: how many items are picked, and Forward, Quote, Discuss in chat, the forks from the last one, Copy and Cancel.
 *
 * It stands in place of the box a message is written in while items are being picked. Forward sends
 * them in the dialog the chats use, to chats, people and agents' threads; Quote puts their words into the
 * box that is written to the agent, as a quotation; Discuss in chat leaves them as live quotes in the box
 * of the thread's chat. Fork from here makes the fork at once, Fork from here to… plans it, and Side
 * thread from here asks on the side, each from the end of the last item's turn (from before a person's
 * message). Copy copies them as Markdown, in the written form of a conversation: what was said as text,
 * a command and its output in a fenced block, a change as a diff; "Copy as…" has plain text, JSON (the
 * items as the API gives them) and a link that opens the thread with them lit.
 */
import { ChevronDown, GitFork, MessageSquareDashed, MessageSquareQuote, Quote } from 'lucide-react';
import { useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { FORWARD_MAX_ITEMS } from '@/api/chats';
import { openChange } from '@/features/change/target';
import { chatOfNode, discussInChat } from '@/features/chat/discuss';
import { composerWriter } from '@/features/composer/Draft';
import { ForwardDialog, type ForwardSource } from '@/features/forward/ForwardDialog';
import { SelectionBar } from '@/features/forward/Selection';
import { openSideAsk } from '@/features/side-threads/ask';
import { useForkNow } from '@/features/thread/forkNow';
import { itemsLink, itemText } from '@/features/timeline/ItemActions';
import type { Item } from '@/generated/api-types';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadView } from '@/realtime/stores/thread';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { Button } from '@/ui/Button';
import { copyText } from '@/ui/clipboard';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';

/** A fenced block that no run of backticks in its body can close. */
function fenced(language: string, body: string): string {
  const longest = Math.max(2, ...Array.from(body.matchAll(/`+/g), (run) => run[0].length));
  const marks = '`'.repeat(longest + 1);
  return `${marks}${language}\n${body}\n${marks}`;
}

/** An item as Markdown: a command and its output in a fenced block, a change as a diff, a step that is neither by its title, and what was said as its text. */
export function itemMarkdown(item: Item): string {
  if (item.kind === 'tool_call') {
    const tool = item.payload.tool;
    if (tool.kind === 'command') return fenced('sh', `$ ${tool.command}\n${tool.output.inline}`.trimEnd());
    if (tool.kind === 'file_change') {
      return tool.changes.map((change) => (change.diff === null ? `\`${change.path}\`` : fenced('diff', change.diff))).join('\n\n');
    }
    return `**${item.payload.title}**`;
  }
  if (item.kind === 'announcement') return `**${item.payload.title}**\n\n${item.payload.body}`;
  return itemText(item);
}

/** What is picked as Markdown, in its order, one block after the other. */
export function selectionMarkdown(items: readonly Item[]): string {
  return items
    .map(itemMarkdown)
    .filter((one) => one.trim() !== '')
    .join('\n\n');
}

export interface TimelineSelectionBarProps {
  threadId: string;
  /** What is picked, in the order of the conversation. */
  ids: readonly string[];
  /** Whether the person may write to the agent: Quote puts words into that box. */
  canQuote: boolean;
  onCancel: () => void;
}

export function TimelineSelectionBar({ threadId, ids, canQuote, onCancel }: TimelineSelectionBarProps) {
  const route = useContext(ViewRouteContext);
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(read);
  const [forwarding, setForwarding] = useState<ForwardSource | null>(null);
  const thread = view.thread;
  const items = ids.flatMap((id) => view.items.get(id) ?? []);
  const words = items.map(itemText).filter((one) => one.trim() !== '').join('\n\n');
  const quickFork = useForkNow(threadId);

  // The forks start from the last item picked: through its turn, or through the turn before a person's message, whose
  // words then stand in the new composer.
  const last = items[items.length - 1];
  const lastTurnId = last?.turn_id ?? null;
  const lastTurn = lastTurnId === null ? undefined : view.turns.get(lastTurnId);
  const through = last === undefined || lastTurn === undefined ? null : last.kind === 'user_message' ? lastTurn.ordinal - 1 : lastTurn.ordinal;
  const said = last?.kind !== 'user_message' ? '' : itemText(last);
  const forks: MenuEntry[] = [];
  if (thread !== null && through !== null) {
    const target = { kind: 'thread' as const, id: threadId, name: thread.name };
    if (said === '') {
      forks.push({
        id: 'fork-now',
        label: t('timeline.selection.forkNow'),
        icon: <GitFork aria-hidden />,
        disabled: quickFork.pending ? t('change.forkNow.making') : false,
        // The bar stays while the fork is made, so that the new thread opens when it is there.
        onSelect: () => quickFork.start(through),
      });
    }
    forks.push({
      id: 'fork-to',
      label: t('timeline.selection.forkTo'),
      icon: <GitFork aria-hidden />,
      onSelect: () => {
        openChange(target, { kind: 'fork', toTurn: through, ...(said === '' ? {} : { say: said }) });
        onCancel();
      },
    });
    if (canQuote && thread.side_of === null && thread.sides?.allowed !== false) {
      forks.push({
        id: 'side',
        label: t('timeline.selection.side'),
        icon: <MessageSquareDashed aria-hidden />,
        onSelect: () => {
          openSideAsk({ threadId, threadName: thread.name, toTurn: through, text: '', read });
          onCancel();
        },
      });
    }
  }
  const copies: MenuEntry[] = [
    { id: 'copy-plain', label: t('timeline.selection.copyPlain'), onSelect: () => void copyText(words) },
    { id: 'copy-json', label: t('timeline.selection.copyJson'), onSelect: () => void copyText(JSON.stringify(items, null, 2)) },
    { id: 'copy-link', label: t('timeline.selection.copyLink'), onSelect: () => void copyText(itemsLink(threadId, ids)) },
  ];

  const forward = () => {
    const chatId = chatOfNode(threadId);
    if (thread === null || chatId === null) {
      toast({ tone: 'danger', title: t('timeline.actions.noChat') });
      return;
    }
    setForwarding({
      kind: 'thread',
      threadId,
      orgId: thread.org_id,
      label: thread.name,
      threadChat: { chatId, label: t('chat.title.node', { name: thread.name, kind: t('chat.kind.thread') }) },
      itemIds: ids,
      preview: items.map((item) => ({
        id: item.id,
        author: item.author?.display_name ?? thread.binding?.agent.name ?? '',
        at: item.created_at,
        text: itemText(item).trim(),
        ...(item.author === null ? {} : { avatar: item.author.avatar }),
      })),
    });
  };

  return (
    <>
      <SelectionBar
        count={ids.length}
        most={FORWARD_MAX_ITEMS}
        label={t('timeline.selection.label')}
        tooManyText={t('timeline.selection.tooMany', { count: FORWARD_MAX_ITEMS })}
        onForward={forward}
        onCopy={() => void copyText(selectionMarkdown(items))}
        onCancel={onCancel}
        extra={
          <>
            <Button
              size="sm"
              icon={<Quote aria-hidden />}
              disabled={!canQuote || words === ''}
              onClick={() => {
                composerWriter.quote(threadId, words);
                onCancel();
              }}
            >
              {t('timeline.selection.quote')}
            </Button>
            <Button
              size="sm"
              icon={<MessageSquareQuote aria-hidden />}
              disabled={ids.length === 0}
              onClick={() => {
                if (discussInChat(threadId, ids, route)) onCancel();
                else toast({ tone: 'danger', title: t('timeline.actions.noChat') });
              }}
            >
              {t('timeline.selection.discuss')}
            </Button>
            {forks.length > 0 && (
              <Menu
                label={t('timeline.selection.forks')}
                entries={forks}
                trigger={
                  <Button size="sm" icon={<GitFork aria-hidden />} pending={quickFork.pending}>
                    {t('timeline.selection.forks')}
                  </Button>
                }
              />
            )}
            <Menu
              label={t('timeline.selection.copyAs')}
              entries={copies}
              trigger={
                <Button size="sm" icon={<ChevronDown aria-hidden />} disabled={ids.length === 0}>
                  {t('timeline.selection.copyAs')}
                </Button>
              }
            />
          </>
        }
      />
      <ForwardDialog
        source={forwarding}
        onClose={() => setForwarding(null)}
        onForwarded={() => {
          setForwarding(null);
          onCancel();
        }}
      />
    </>
  );
}
