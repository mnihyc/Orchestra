/** A document's description: written under its title by whoever may edit it and sent as it rests, read as it is by
 * the others, and its first line beside the title where documents are listed. */
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import { DescriptionField } from '@/features/documents/Document';
import { DocumentRow } from '@/features/documents/DocumentRow';
import type { DocumentRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ describe: vi.fn<(...args: Parameters<typeof Documents.describeDocument>) => Promise<unknown>>() }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof Documents>(), describeDocument: calls.describe }));
vi.mock('@tanstack/react-router', () => ({
  Link: ({ children, className }: { children?: ReactNode; className?: string }) => <a href="/doc" className={className}>{children}</a>,
}));

const fonts = Object.getOwnPropertyDescriptor(document, 'fonts');
beforeEach(() => {
  vi.clearAllMocks();
  calls.describe.mockResolvedValue(undefined);
  Object.defineProperty(document, 'fonts', { configurable: true, value: { ready: Promise.resolve(), addEventListener: vi.fn(), removeEventListener: vi.fn() } });
});
afterEach(async () => {
  cleanup();
  if (fonts === undefined) Reflect.deleteProperty(document, 'fonts'); else Object.defineProperty(document, 'fonts', fonts);
  await setLocale('en');
});

it.each(['en', 'zh-CN'] as const)('an editor writes the description in place and it is sent once it rests (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const body = vi.fn();
  show(<DescriptionField doc={{ id: 'doc-1', description: '' }} editable onEnter={body} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.descriptionLabel') });
  expect(field).toHaveAttribute('placeholder', i18n.t('documents.page.descriptionPlaceholder'));
  await user.click(field);
  await user.keyboard('How we ship{Shift>}{Enter}{/Shift}and roll back');
  expect(field).toHaveValue('How we ship\nand roll back');
  expect(body).not.toHaveBeenCalled();
  await waitFor(() => expect(calls.describe).toHaveBeenCalledExactlyOnceWith('doc-1', 'How we ship\nand roll back'));
  await user.keyboard('{Enter}');
  expect(body).toHaveBeenCalledTimes(1);
  expect(field).toHaveValue('How we ship\nand roll back');
  fireEvent.keyDown(field, { key: 'Enter', isComposing: true });
  expect(body).toHaveBeenCalledTimes(1);
});

it('a description somebody else wrote arrives where the field is not being written in', () => {
  const view = show(<DescriptionField doc={{ id: 'doc-1', description: 'Old' }} editable onEnter={vi.fn()} />);
  view.rerender(<DescriptionField doc={{ id: 'doc-1', description: 'New from Bo' }} editable onEnter={vi.fn()} />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.page.descriptionLabel') })).toHaveValue('New from Bo');
  expect(calls.describe).not.toHaveBeenCalled();
});

it('a reader sees the description as written, and nothing where there is none', () => {
  const view = show(<DescriptionField doc={{ id: 'doc-1', description: 'How we ship\nand roll back' }} editable={false} onEnter={vi.fn()} />);
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(view.container.querySelector('[data-document-description]')?.textContent).toBe('How we ship\nand roll back');
  view.rerender(<DescriptionField doc={{ id: 'doc-1', description: '  ' }} editable={false} onEnter={vi.fn()} />);
  expect(view.container.querySelector('[data-document-description]')).toBeNull();
});

const person = { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 3 } } as const;
const listed = (line: string): DocumentRef => ({
  id: 'doc-1', title: 'Rollout plan', description: '', description_source: 'auto', description_line: line,
  node: { id: 'topic-1', kind: 'topic', name: 'Plans', redacted: false }, owner: person, owners: 1, created_by: person,
  page: 'follows', seq: 3, block_count: 2, bytes: 40, files_bytes: 0, version: 0, open_discussions: 0,
  updated_at: '2026-10-08T17:04:05.123Z', updated_by: person, discussion_level: 'involved', seen_seq: 3, archived: false, deleted: false,
});

it('a list shows the first line of what a document is about beside its title, and no empty line', () => {
  const view = show(<ul><DocumentRow doc={listed('Ship the fix behind the flag.')} here="topic-1" where={false} me="ada" /></ul>);
  expect(view.container.querySelector('[data-document-line]')?.textContent).toBe('Ship the fix behind the flag.');
  view.rerender(<ul><DocumentRow doc={listed('')} here="topic-1" where={false} me="ada" /></ul>);
  expect(view.container.querySelector('[data-document-line]')).toBeNull();
});
