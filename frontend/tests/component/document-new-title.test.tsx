/** R1 W13: one press creates the page; its actual title, not a preliminary dialog, receives typing. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { StrictMode } from 'react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as Router from '@tanstack/react-router';
import { TitleField } from '@/features/documents/Document';
import { NameNewDocument } from '@/features/documents/NameNewDocument';
import { useMakeDocument } from '@/features/documents/useMakeDocument';
import { NewDocumentDialog } from '@/features/navigation/NodeDialogs';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { show } from './support';

// These units consume only the created identity or the completion of a rename; native route tests own full DTOs.
const calls = vi.hoisted(() => ({ create: vi.fn<(...args: Parameters<typeof Documents.createDocument>) => Promise<{ id: string }>>(),
  rename: vi.fn<(...args: Parameters<typeof Documents.renameDocument>) => Promise<unknown>>(), navigate: vi.fn<(_target: unknown) => Promise<void>>() }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof Documents>(), createDocument: calls.create, renameDocument: calls.rename }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useNavigate: () => calls.navigate, useBlocker: () => undefined }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
const fonts = Object.getOwnPropertyDescriptor(document, 'fonts');
function Create() {
  const make = useMakeDocument({ id: 'topic', access: { permissions: ['doc.create', 'doc.edit'] } });
  return <NameNewDocument trigger={<Button pending={make.isPending}>{i18n.t('documents.list.new')}</Button>} pending={make.isPending}
    onMake={(title) => make.mutate(title === undefined ? {} : { title })} />;
}
beforeEach(() => {
  vi.clearAllMocks(); calls.rename.mockResolvedValue(undefined); calls.navigate.mockResolvedValue(undefined);
  Object.defineProperty(document, 'fonts', { configurable: true, value: { ready: Promise.resolve(), addEventListener: vi.fn(), removeEventListener: vi.fn() } });
});
afterEach(async () => {
  cleanup(); useAppearance.setState(before);
  if (fonts === undefined) Reflect.deleteProperty(document, 'fonts'); else Object.defineProperty(document, 'fonts', fonts);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

it.each(modes)('creates once immediately and offers the current page title once in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.classList.toggle('dark', theme === 'dark'); document.documentElement.dataset.theme = theme;
  // RTL drains an additional zero-delay task after each user interaction. Keep those real,
  // and wait for the actual autosave acknowledgement rather than freezing its drain timer.
  const user = userEvent.setup();
  let finish: ((created: { id: string }) => void) | undefined;
  calls.create.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const create = show(<Create />);
  await user.click(screen.getByRole('button', { name: i18n.t('documents.list.new') }));
  expect(calls.create).toHaveBeenCalledExactlyOnceWith('topic', { title: i18n.t('documents.page.untitled'), access: 'follows' }, expect.any(String));
  expect(screen.queryByRole('dialog')).toBeNull();
  expect(screen.queryByRole('textbox')).toBeNull();
  expect(calls.navigate).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('documents.list.new') }));
  expect(calls.create).toHaveBeenCalledTimes(1);
  await act(async () => { finish?.({ id: 'made' }); await Promise.resolve(); });
  await waitFor(() => expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/doc/$documentId', params: { documentId: 'made' }, search: { new: true } }));
  create.unmount();
  const offered = vi.fn();
  const body = vi.fn();
  const title = { id: 'made', title: i18n.t('documents.page.untitled') };
  const page = show(<TitleField doc={title} editable fresh onOffered={offered} onEnter={body} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') });
  expect(field).toHaveFocus();
  if (!(field instanceof HTMLTextAreaElement)) throw new Error('the title is the real editable field');
  expect([field.selectionStart, field.selectionEnd]).toEqual([0, title.title.length]);
  expect(offered).toHaveBeenCalledTimes(1);
  await user.keyboard(locale === 'en' ? 'Launch checklist' : '发布清单');
  const name = locale === 'en' ? 'Launch checklist' : '发布清单';
  expect(field).toHaveValue(name);
  await waitFor(() => expect(calls.rename).toHaveBeenCalledExactlyOnceWith('made', name));
  fireEvent.keyDown(field, { key: 'Enter', isComposing: true });
  expect(body).not.toHaveBeenCalled();
  await user.keyboard('{Enter}');
  expect(body).toHaveBeenCalledTimes(1);
  page.rerender(<TitleField doc={title} editable fresh onOffered={offered} onEnter={body} />);
  expect(offered).toHaveBeenCalledTimes(1);
});

it('never focuses or renames a title that this reader may not edit', () => {
  const offered = vi.fn();
  show(<TitleField doc={{ id: 'read-only', title: 'A shared page' }} editable={false} fresh onOffered={offered} onEnter={vi.fn()} />);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') })).not.toHaveFocus();
  expect(offered).not.toHaveBeenCalled();
  expect(calls.rename).not.toHaveBeenCalled();
});

it.each([{ pending: true, cancelled: false }, { pending: false, cancelled: true }])('honors an existing control’s pending/refusal state ($pending/$cancelled)', async ({ pending, cancelled }) => {
  const make = vi.fn();
  const user = userEvent.setup();
  show(<NameNewDocument pending={pending} onMake={make} trigger={<Button onClick={(event) => { if (cancelled) event.preventDefault(); }}>{i18n.t('documents.list.new')}</Button>} />);
  const control = screen.getByRole('button', { name: i18n.t('documents.list.new') });
  expect(screen.getAllByRole('button')).toHaveLength(1);
  if (pending) expect(control).toHaveAttribute('aria-busy', 'true');
  await user.click(control);
  expect(make).not.toHaveBeenCalled();
});

it('also creates once from the navigation menu, including Strict Mode effect replay, without a name dialog', async () => {
  let finish: ((created: { id: string }) => void) | undefined;
  calls.create.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
  const close = vi.fn();
  show(<StrictMode><NewDocumentDialog parent={{ id: 'topic', name: 'Release', kind: 'topic', parentId: 'project', archived: false, status: 'open', folder: false }} open onClose={close} /></StrictMode>);
  await waitFor(() => expect(calls.create).toHaveBeenCalledExactlyOnceWith('topic', { title: i18n.t('documents.page.untitled'), access: 'follows' }, expect.any(String)));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  await act(async () => { finish?.({ id: 'from-menu' }); await Promise.resolve(); });
  await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  expect(calls.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/doc/$documentId', params: { documentId: 'from-menu' }, search: { new: true } });
});
