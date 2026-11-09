/** A message menu copies the producer's shared-reader ordinal and retains old-message fallback. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type { MessageActions } from '@/features/chat/context';
import { messageEntries } from '@/features/chat/Message';
import { messageReferenceText } from '@/features/chat/messageReference';
import { i18n, setLocale } from '@/i18n';
import { Menu } from '@/ui/Menu';
import { ADA, chat, message } from '../doubles/chat-rows';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

function actions(copy: (text: string) => void, name: string): MessageActions {
  return { react: vi.fn(), reply: vi.fn(), edit: vi.fn(), remove: vi.fn(), pin: vi.fn(), forward: vi.fn(), select: vi.fn(), copyText: vi.fn(), copyLink: vi.fn(), jump: vi.fn(),
    copyReference: (row) => { const written = messageReferenceText(row, name); if (written !== null) copy(written); },
  };
}

it.each(['en', 'zh-CN'] as const)('enables Copy reference from the native number in %s', async (locale) => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Project chat' : '项目讨论';
  const row = { ...message(20), reference_number: 3 };
  const copy = vi.fn();
  show(<Menu label="Actions" trigger={<button type="button">Actions</button>}
    entries={messageEntries(i18n.t as (key: string) => string, chat(), row, ADA.id, actions(copy, name))} />);
  await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
  await userEvent.click(screen.getByRole('menuitem', { name: i18n.t('people.reference.copy') }));
  expect(copy).toHaveBeenCalledOnce();
  expect(copy).toHaveBeenCalledWith(`[${name}](id:${row.chat_id}#3)`);
});

it('keeps Copy reference disabled for an older message with no producer number', async () => {
  const copy = vi.fn();
  show(<Menu label="Actions" trigger={<button type="button">Actions</button>}
    entries={messageEntries(i18n.t as (key: string) => string, chat(), message(20), ADA.id, actions(copy, 'Project chat'))} />);
  await userEvent.click(screen.getByRole('button', { name: 'Actions' }));
  const entry = screen.getByRole('menuitem', { name: new RegExp(i18n.t('people.reference.copy')) });
  expect(entry).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(entry);
  expect(copy).not.toHaveBeenCalled();
});
