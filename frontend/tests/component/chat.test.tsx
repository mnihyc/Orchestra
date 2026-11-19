/** What is typed in a chat and what stands under a message: the keys of the box a message is written in (an input method that is composing never sends), who may be named, and what each message offers its reader. */
import { QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useRef, useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { queryClient } from '@/api/query';
import type * as Acts from '@/features/chat/acts';
import type * as Uploads from '@/lib/uploads';
import type * as PeopleApi from '@/api/people';
import { discard, edit, resend, send } from '@/features/chat/acts';
import { ChatContext, type ChatContextValue, type MessageActions } from '@/features/chat/context';
import { chatPageTitle } from '@/features/chat/ChatView';
import { Composer } from '@/features/chat/Composer';
import { i18n, setLocale } from '@/i18n';
import type { Draft } from '@/features/chat/draft';
import { Message, OutgoingMessage } from '@/features/chat/Message';
import type { Chat, ChatMessage, ChatPart, UploadedBlob } from '@/generated/api-types';
import type * as ChatStore from '@/realtime/stores/chat';

import { ADA, chat, DORA, GRACE, message } from '../doubles/chat-rows';
import { show } from './support';

const upload = vi.hoisted(() => ({ run: vi.fn() }));
vi.mock('@/api/people', async (original) => ({
  ...(await original<typeof PeopleApi>()),
  mentionChoicesQuery: (id: string, q: string) => ({ queryKey:['chat-mentions',id,q], queryFn:() => Promise.resolve({
    items:[DORA,GRACE].filter((user) => user.username.includes(q) || user.display_name.toLowerCase().includes(q.toLowerCase())).map((user) => ({target:{kind:'user',id:user.id},name:user.username,user,count:1,available:true,reason:null,thread:null,agent:null,state:null,started_at:null})), next_cursor:null,followers:2,
  }) }),
  mentionAudienceQuery: (id: string, parts: ChatPart[]) => ({ queryKey:['chat-mention-audience',id,parts], queryFn:() => Promise.resolve({count:2}) }),
}));
vi.mock('@/lib/uploads', async (original) => ({
  ...(await original<typeof Uploads>()),
  UploadTask: class { run = upload.run; },
  attachToChat: (_id: string, blob: UploadedBlob): Promise<ChatPart> => Promise.resolve({ type: 'image', blob: blob.blob, name: blob.name, media_type: 'image/png' }),
}));

vi.mock('@/features/chat/acts', async (original) => ({
  ...(await original<typeof Acts>()),
  send: vi.fn(),
  edit: vi.fn(),
  discard: vi.fn(),
  resend: vi.fn(),
}));
vi.mock('@/realtime/stores/chat', async (original) => ({
  ...(await original<typeof ChatStore>()),
  tellTyping: vi.fn(),
}));

// Each case selects its language explicitly; an earlier authored-name case may leave Chinese active.
beforeEach(async () => { await setLocale('en'); });

const GROUP = chat({ kind: 'group', node: null, name: 'Release crew', members: [ADA, DORA, GRACE] });

it.each([['en', 'Group', 'Chat · Group'], ['zh-CN', '群聊', '聊天 · 群聊']] as const)('keeps an authored group name distinct from its unnamed fallback in %s', async (locale, name, title) => {
  await setLocale(locale);
  expect(chatPageTitle(chat({ kind: 'group', node: null, name, members: [] }), ADA.id, i18n.t)).toBe(title);
});

it('shows both people in group events as interactive chips', () => {
  const event = message(1, { kind: 'system', system: { event: 'member_added', by: ADA, user: DORA, name: null } });
  show(inChat({ chat: GROUP }, <ol><Message message={event} head /></ol>));
  expect(screen.getByRole('button', { name: ADA.display_name })).toBeVisible();
  expect(screen.getByRole('button', { name: DORA.display_name })).toBeVisible();
  expect(screen.getByText(/added/)).toBeVisible();
});

it('keeps pinned and replied-to people interactive and the original message reachable', async () => {
  const jump = vi.fn();
  const shown = message(2, {
    pinned: { at: '2026-10-01T09:06:00.000Z', by: GRACE },
    reply_to: { id: 'original', author: DORA, excerpt: 'Earlier words', deleted: false },
  });
  show(inChat({ chat: GROUP, acts: { ...acts(), jump } }, <ol><Message message={shown} head /></ol>));
  expect(screen.getByRole('button', { name: GRACE.display_name })).toBeVisible();
  expect(screen.getByRole('button', { name: DORA.display_name })).toBeVisible();
  await userEvent.setup().click(screen.getByRole('button', { name: /Open original message.*Earlier words/ }));
  expect(jump).toHaveBeenCalledWith('original');
});

function acts(): MessageActions {
  return {
    react: vi.fn(),
    reply: vi.fn(),
    edit: vi.fn(),
    remove: vi.fn(),
    pin: vi.fn(),
    forward: vi.fn(),
    select: vi.fn(),
    copyText: vi.fn(),
    copyLink: vi.fn(),
    jump: vi.fn(),
  };
}

function inChat(value: Partial<ChatContextValue> & { chat: Chat }, children: ReactNode): ReactNode {
  const context: ChatContextValue = {
    me: ADA.id,
    people: new Map([ADA, DORA, GRACE].map((user) => [user.id, user])),
    acts: acts(),
    editing: null,
    setEditing: vi.fn(),
    selection: { active: false, has: () => false, toggle: vi.fn() },
    lit: null,
    ...value,
  };
  return (
    <QueryClientProvider client={queryClient}>
      <ChatContext.Provider value={context}>{children}</ChatContext.Provider>
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.mocked(send).mockReset();
  vi.mocked(edit).mockReset().mockResolvedValue(true);
  vi.mocked(discard).mockReset();
  vi.mocked(resend).mockReset();
});

// ---------------------------------------------------------------- the box a message is written in

function Box({ onEditLast = vi.fn(), start = '', replyTo = null, replyingTo = null, saving = 'idle' }: { onEditLast?: () => void; start?: string; replyTo?: string | null; replyingTo?: ChatMessage | null; saving?: Draft['saving'] }) {
  const [text, setText] = useState(start);
  const [reply, setReply] = useState<string | null>(replyTo);
  const input = useRef<HTMLTextAreaElement>(null);
  const [attachments, setAttachments] = useState<ChatPart[]>([]);
  const draft: Draft = {
    text,
    replyTo: reply,
    quotes: [],
    attachments,
    setText,
    setReplyTo: setReply,
    addQuotes: vi.fn(),
    removeQuote: vi.fn(),
    addAttachment: (part) => setAttachments((parts) => [...parts, part]),
    removeAttachment: (index) => setAttachments((parts) => parts.filter((_, at) => at !== index)),
    clear: () => {
      setText('');
      setReply(null);
      setAttachments([]);
    },
    saving,
  };
  return inChat(
    { chat: GROUP },
    <Composer chat={GROUP} draft={draft} replyingTo={replyingTo} inputRef={input} onEditLast={onEditLast} placeholder="Message Release crew…" />,
  );
}

const box = () => screen.getByRole('textbox', { name: 'Message Release crew…' });

describe('the box a message is written in', () => {
  it('keeps the reply author available as a person chip', () => {
    show(<Box replyingTo={message(1, { author: DORA })} />);
    expect(screen.getByRole('button', { name: DORA.display_name })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Cancel reply' })).toBeVisible();
  });
  for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`keeps a short placeholder, the full destination name and multiline text at ${locale}/${theme}`, async () => {
    const beforeTheme = document.documentElement.getAttribute('data-theme');
    try {
      await setLocale(locale); document.documentElement.dataset.theme = theme;
      const user = userEvent.setup(); show(<Box />);
      const input = box();
      expect(input).toHaveAttribute('placeholder', i18n.t('chat.composer.placeholder'));
      expect(input).toHaveAccessibleName('Message Release crew…');
      await user.type(input, 'First line{Shift>}{Enter}{/Shift}Second line');
      expect(input).toHaveValue('First line\nSecond line');
      expect(send).not.toHaveBeenCalled();
    } finally {
      if (beforeTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.dataset.theme = beforeTheme;
      await setLocale('en');
    }
  });
  it('advertises only real files and keeps meaningful draft status in its live region', () => {
    const view = show(<Box />);
    expect(screen.queryByRole('list', { name: 'Files being attached' })).not.toBeInTheDocument();
    expect(screen.queryByRole('list', { name: 'Attached files' })).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
    view.rerender(<Box saving="saving" />);
    expect(screen.getByRole('status')).toHaveTextContent('Saving draft…');
    view.rerender(<Box saving="failed" />);
    expect(screen.getByRole('status')).toHaveTextContent('Draft not saved');
    view.rerender(<Box saving="idle" />);
    expect(screen.getByRole('status')).toBeEmptyDOMElement();
  });

  it('keeps attachments, their question, emoji and Send together and reachable from the message by keyboard', async () => {
    const user = userEvent.setup();
    show(<Box start="Ready to send" />);
    const controls = screen.getByRole('group', { name: 'Chat message controls' });
    const buttons = within(controls).getAllByRole('button');
    // Attach, the attachments' "?" (where sent files go), emoji, Send.
    expect(buttons).toHaveLength(4);
    await user.click(box());
    for (const button of buttons) {
      await user.tab();
      expect(button).toHaveFocus();
      expect(button).toHaveAccessibleName();
    }
    await user.keyboard('{Enter}');
    expect(send).toHaveBeenCalledWith(GROUP.id, [{ type: 'text', text: 'Ready to send' }], null);
  });

  it('sends on Enter, leaves the box empty, and takes Shift+Enter for a new line', async () => {
    const user = userEvent.setup();
    show(<Box />);

    await user.type(box(), 'first line{Shift>}{Enter}{/Shift}second line');
    expect(box()).toHaveValue('first line\nsecond line');
    expect(send).not.toHaveBeenCalled();

    await user.keyboard('{Enter}');
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenCalledWith(GROUP.id, [{ type: 'text', text: 'first line\nsecond line' }], null);
    expect(box()).toHaveValue('');
  });

  it('sends nothing for a box of blanks, and the send button waits until there is something to send', async () => {
    const user = userEvent.setup();
    show(<Box />);
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();

    await user.type(box(), '   {Enter}');
    expect(send).not.toHaveBeenCalled();

    await user.type(box(), 'hi');
    expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: 'Send message' }));
    expect(send).toHaveBeenCalledWith(GROUP.id, [{ type: 'text', text: 'hi' }], null);
  });

  it('never sends on the Enter that ends a composition of an input method, however the browser reports it', () => {
    show(<Box start="nihao" />);
    fireEvent.keyDown(box(), { key: 'Enter', isComposing: true });
    // Safari reports the key that ends a composition after the composition is over, as 229.
    fireEvent.keyDown(box(), { key: 'Enter', keyCode: 229 });
    expect(send).not.toHaveBeenCalled();
    expect(box()).toHaveValue('nihao');

    fireEvent.keyDown(box(), { key: 'Enter' });
    expect(send).toHaveBeenCalledTimes(1);
  });

  it('takes the reply with it when it sends, and Escape lets the reply go and keeps the text', async () => {
    const user = userEvent.setup();
    const { unmount } = show(<Box replyTo="the-message" start="agreed" />);
    await user.click(box());
    await user.keyboard('{Enter}');
    expect(send).toHaveBeenLastCalledWith(GROUP.id, [{ type: 'text', text: 'agreed' }], 'the-message');
    unmount();

    show(<Box replyTo="the-message" start="agreed" />);
    await user.click(box());
    await user.keyboard('{Escape}{Enter}');
    expect(send).toHaveBeenLastCalledWith(GROUP.id, [{ type: 'text', text: 'agreed' }], null);
  });

  it('offers the people of the chat after an @ with everybody first, and puts the one chosen in the text', async () => {
    const user = userEvent.setup();
    show(<Box />);

    await user.type(box(), 'Thanks @');
    const list = screen.getByRole('listbox', { name: 'People to mention' });
    const names = within(list).getAllByRole('option').map((option) => option.textContent);
    expect(names[0]).toContain('@all');
    expect(names.join(' ')).toContain('Dora Developer');
    expect(names.join(' ')).toContain('Grace Hopper');
    // The reader is not offered themselves.
    expect(names.join(' ')).not.toContain('Ada Lovelace');

    await user.keyboard('{ArrowDown}{Enter}');
    expect(box()).toHaveValue('Thanks @dora ');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    // The Enter that chose somebody did not send anything.
    expect(send).not.toHaveBeenCalled();
  });

  it('narrows the people by what is typed after the @, and lets Escape close the list without losing the text', async () => {
    const user = userEvent.setup();
    show(<Box />);

    await user.type(box(), '@gr');
    expect(within(screen.getByRole('listbox')).getAllByRole('option')).toHaveLength(1);
    expect(screen.getByRole('option', { name: /Grace Hopper/ })).toBeInTheDocument();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
    expect(box()).toHaveValue('@gr');
  });

  it('does not take an address for a mention', async () => {
    const user = userEvent.setup();
    show(<Box />);
    await user.type(box(), 'write to ada@example.org');
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('goes to the last message of the reader on Up in an empty box, and not when there is something typed', async () => {
    const user = userEvent.setup();
    const onEditLast = vi.fn();
    show(<Box onEditLast={onEditLast} />);
    await user.click(box());
    await user.keyboard('{ArrowUp}');
    expect(onEditLast).toHaveBeenCalledTimes(1);

    await user.type(box(), 'x');
    await user.keyboard('{ArrowUp}');
    expect(onEditLast).toHaveBeenCalledTimes(1);
  });

  it('keeps the text and waits for a dropped image before sending its completed part', async () => {
    let finish: (blob: UploadedBlob) => void = () => undefined;
    upload.run.mockReturnValueOnce(new Promise<UploadedBlob>((resolve) => { finish = resolve; }));
    show(<Box start="look at this" />);
    const file = new File(['x'], 'x.png', { type: 'image/png' });
    fireEvent.drop(box(), { dataTransfer: { types: ['Files'], files: [file], items: [{ kind: 'file', type: 'image/png', getAsFile: () => file }], getData: () => '' } });
    // A drop is read first and then becomes the attachment's chip (the one gesture of putting things in).
    expect(await screen.findByText('x.png')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
    expect(box()).toHaveValue('look at this');
    act(() => { finish({ blob: 'a'.repeat(64), name: 'x.png', size: 1, media_type: 'image/png' }); });
    await waitFor(() => { expect(screen.getByRole('button', { name: 'Send message' })).toBeEnabled(); });
    await userEvent.click(screen.getByRole('button', { name: 'Send message' }));
    expect(send).toHaveBeenCalledWith(GROUP.id, [{ type: 'text', text: 'look at this' }, { type: 'image', blob: 'a'.repeat(64), name: 'x.png', media_type: 'image/png' }], null);
  });
});

// ---------------------------------------------------------------- one message

function drawn(shown: ChatMessage, options: { chat?: Chat; me?: string | null; editing?: string | null } = {}) {
  const the = options.chat ?? GROUP;
  const context = { chat: the, ...(options.me === undefined ? {} : { me: options.me }), ...(options.editing === undefined ? {} : { editing: options.editing }) };
  return show(inChat(context, <ol>{<Message message={shown} head />}</ol>));
}

describe('a message', () => {
  it('keeps Review, Try again and Discard as named, independent controls after a send fails', async () => {
    const user = userEvent.setup(), restore = vi.fn();
    show(inChat({ chat: GROUP }, <ol><OutgoingMessage chatId={GROUP.id} author={ADA} onRestore={restore}
      sent={{ clientId: 'failed-layout', parts: [{ type: 'text', text: 'Keep this draft' }], replyTo: null, at: Date.now(), state: 'failed', error: null, agentReview: 'reviewed-input' }} /></ol>));
    expect(screen.getByText('Keep this draft')).toBeVisible();
    expect(screen.getAllByRole('button').map((button) => button.getAttribute('aria-label') ?? button.textContent)).toEqual([
      ADA.display_name, 'Review issue',
    ]);
    await user.click(screen.getByRole('button', { name: 'Review issue' }));
    await user.click(screen.getByRole('button', { name: 'Review and send again' }));
    expect(restore).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: 'Try again' }));
    expect(resend).toHaveBeenCalledWith(GROUP.id, 'failed-layout');
    await user.click(screen.getByRole('button', { name: 'Discard' }));
    expect(discard).toHaveBeenCalledWith(GROUP.id, 'failed-layout');
  });

  it('shows who wrote it, what, and that it was edited', () => {
    drawn(message(1, { author: DORA, parts: [{ type: 'text', text: 'We can ship it' }], edited_at: '2026-10-01T09:05:00.000Z' }));
    expect(screen.getByText('Dora Developer')).toBeInTheDocument();
    expect(screen.getByText('We can ship it')).toBeInTheDocument();
    expect(screen.getByText('(edited)')).toBeInTheDocument();
  });

  it('shows a deleted message as such, with nothing of what it said and nothing to react to', () => {
    drawn(message(1, { author: DORA, deleted: true, parts: [] }));
    expect(screen.getByText('Message deleted')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'React' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Reply' })).not.toBeInTheDocument();
  });

  it('rings the reactions the reader made, and tells what pressing one does', async () => {
    const user = userEvent.setup();
    const the = acts();
    show(
      inChat(
        { chat: GROUP, acts: the },
        <ol>
          <Message
            head
            message={message(1, {
              author: DORA,
              reactions: [
                { emoji: '👍', users: [ADA.id, GRACE.id] },
                { emoji: '🎉', users: [GRACE.id] },
              ],
            })}
          />
        </ol>,
      ),
    );
    const mine = screen.getByRole('button', { name: /^👍 2 reactions/ });
    const theirs = screen.getByRole('button', { name: /^🎉 1 reaction/ });
    expect(mine).toHaveAttribute('aria-pressed', 'true');
    expect(theirs).toHaveAttribute('aria-pressed', 'false');

    await user.click(theirs);
    expect(the.react).toHaveBeenCalledWith(expect.objectContaining({ id: message(1).id }), '🎉');
  });

  it('turns a mention of the reader into a mark, and leaves what is not a mention as text', () => {
    drawn(message(1, { author: DORA, parts: [{ type: 'text', text: 'Please look @ada, not ada@example.org' }], mentions: [ADA] }));
    expect(screen.getByText('@Ada Lovelace')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '@Ada Lovelace' })).toBeVisible();
    expect(screen.getByText(/ada@example.org/)).toBeInTheDocument();
  });

  it('edits in its place: Enter saves, an Enter that ends a composition does not, and Escape puts it back', async () => {
    const user = userEvent.setup();
    const setEditing = vi.fn();
    const mine = message(1, { parts: [{ type: 'text', text: 'We retry after' }] });
    show(inChat({ chat: GROUP, editing: mine.id, setEditing }, <ol><Message head message={mine} /></ol>));

    const editor = screen.getByRole('textbox', { name: 'Edit message' });
    expect(editor).toHaveValue('We retry after');
    await user.clear(editor);
    await user.type(editor, 'We retry before');
    fireEvent.keyDown(editor, { key: 'Enter', isComposing: true });
    expect(edit).not.toHaveBeenCalled();

    await user.keyboard('{Enter}');
    expect(edit).toHaveBeenCalledWith(GROUP.id, mine.id, [{ type: 'text', text: 'We retry before' }]);
    await vi.waitFor(() => expect(setEditing).toHaveBeenCalledWith(null));

    setEditing.mockClear();
    await user.keyboard('{Escape}');
    expect(setEditing).toHaveBeenCalledWith(null);
  });

  it('does not save an edit that leaves nothing', async () => {
    const user = userEvent.setup();
    const mine = message(1, { parts: [{ type: 'text', text: 'We retry after' }] });
    show(inChat({ chat: GROUP, editing: mine.id }, <ol><Message head message={mine} /></ol>));
    const editor = screen.getByRole('textbox', { name: 'Edit message' });
    await user.clear(editor);
    await user.keyboard('{Enter}');
    expect(edit).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });
});

describe('what a message offers its reader', () => {
  async function menuOf(shown: ChatMessage, options: Parameters<typeof drawn>[1] = {}): Promise<string[]> {
    const user = userEvent.setup();
    drawn(shown, options);
    await user.click(screen.getByRole('button', { name: /^Actions/ }));
    return screen.getAllByRole('menuitem').map((item) => item.textContent);
  }

  it('is, for what the reader wrote, everything but nothing to hide: reply, forward, select, edit, pin, copy, and delete last', async () => {
    const entries = await menuOf(message(1, { author: ADA }), { me: ADA.id });
    expect(entries).toEqual(['Reply', 'Forward…', 'Select', 'Edit', 'Pin', 'Copy text', 'Copy link', 'Delete']);
  });

  it('is, for what another wrote, without edit and delete, unless the reader moderates the chat', async () => {
    const theirs = message(1, { author: DORA });
    expect(await menuOf(theirs, { me: ADA.id })).toEqual(['Reply', 'Forward…', 'Select', 'Pin', 'Copy text', 'Copy link']);
  });

  it('lets a moderator delete what another wrote', async () => {
    const moderated = chat({ kind: 'group', node: null, members: [ADA, DORA], can: { admin_only: [], participating: true, send: true, moderate: true, export: true, reason: null } });
    expect(await menuOf(message(1, { author: DORA }), { me: ADA.id, chat: moderated })).toContain('Delete');
  });

  it('is only to read, forward, select and link in a chat that cannot be written in', async () => {
    const reading = chat({ kind: 'group', node: null, members: [DORA, GRACE], can: { admin_only: [], participating: false, send: false, moderate: false, export: false, reason: 'inspecting' } });
    expect(await menuOf(message(1, { author: DORA }), { me: ADA.id, chat: reading })).toEqual(['Forward…', 'Select', 'Copy text', 'Copy link']);
  });

  it('is only to select and link for a message that was deleted', async () => {
    expect(await menuOf(message(1, { author: ADA, deleted: true, parts: [] }), { me: ADA.id })).toEqual(['Select', 'Copy link']);
  });

  it('puts Unpin where Pin was for a message that is pinned', async () => {
    const pinned = message(1, { author: DORA, pinned: { at: '2026-10-01T09:06:00.000Z', by: GRACE } });
    expect(await menuOf(pinned, { me: ADA.id })).toContain('Unpin');
  });
});
