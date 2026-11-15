/** One press creates the page; its actual title receives typing. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import { StrictMode } from 'react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Documents from '@/api/documents';
import type * as Router from '@tanstack/react-router';
import { TitleField, type DocumentFieldSave } from '@/features/documents/Document';
import { NameNewDocument } from '@/features/documents/NameNewDocument';
import { useMakeDocument } from '@/features/documents/useMakeDocument';
import { NewDocumentDialog } from '@/features/navigation/NodeDialogs';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { deferred } from './deferred';
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

it.each(modes)('keeps a failed title through remote updates and retries until it is acknowledged in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  calls.rename.mockRejectedValueOnce(new NetworkError('offline'));
  const view = show(<TitleField doc={{ id: 'draft-title', title: 'Original' }} editable onEnter={vi.fn()} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') });
  fireEvent.change(field, { target: { value: 'My retained title' } });
  expect(await screen.findByRole('alert', {}, { timeout: 2_000 })).toHaveTextContent(i18n.t('documents.page.titleFailed'));
  view.rerender(<TitleField doc={{ id: 'draft-title', title: 'A teammate’s title' }} editable onEnter={vi.fn()} />);
  expect(field).toHaveValue('My retained title');
  const pending = deferred<unknown>(); calls.rename.mockReturnValueOnce(pending.promise);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.rename).toHaveBeenCalledTimes(2));
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('documents.page.saving'));
  expect(field).toHaveValue('My retained title');
  await act(async () => { pending.resolve(undefined); await pending.promise; });
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
  expect(calls.rename.mock.calls).toEqual([['draft-title', 'My retained title'], ['draft-title', 'My retained title']]);
});

it('sends title edits in order while an earlier save is still awaiting acknowledgement', async () => {
  const pending = deferred<unknown>(); calls.rename.mockReturnValueOnce(pending.promise);
  const view = show(<TitleField doc={{ id: 'ordered-title', title: 'Original' }} editable onEnter={vi.fn()} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') });
  fireEvent.change(field, { target: { value: 'First title' } });
  await waitFor(() => expect(calls.rename).toHaveBeenCalledTimes(1), { timeout: 2_000 });
  fireEvent.change(field, { target: { value: 'Second title' } });
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 800)); });
  expect(calls.rename).toHaveBeenCalledTimes(1);
  view.rerender(<TitleField doc={{ id: 'ordered-title', title: 'First title' }} editable onEnter={vi.fn()} />);
  expect(field).toHaveValue('Second title');
  await act(async () => { pending.resolve(undefined); await pending.promise; });
  await waitFor(() => expect(calls.rename).toHaveBeenCalledTimes(2), { timeout: 2_000 });
  expect(calls.rename.mock.calls).toEqual([['ordered-title', 'First title'], ['ordered-title', 'Second title']]);
});

it('keeps a failed title unsaved for the close decision and lets Save retry the retained text', async () => {
  let guard: DocumentFieldSave | null = null;
  const current = () => { if (guard === null) throw new Error('The title must register its close guard.'); return guard; };
  calls.rename.mockRejectedValueOnce(new NetworkError('offline'));
  show(<TitleField doc={{ id: 'close-title', title: 'Original' }} editable onEnter={vi.fn()} onSaveReady={(next) => { guard = next; }} />);
  fireEvent.change(screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') }), { target: { value: 'Keep before closing' } });
  expect(current().hasUnsent()).toBe(true);
  let saved = false;
  await act(async () => { saved = await current().save(); });
  expect(saved).toBe(false);
  expect(current().hasUnsent()).toBe(true);
  expect(screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') })).toHaveValue('Keep before closing');
  await act(async () => { saved = await current().save(); });
  expect(saved).toBe(true);
  expect(current().hasUnsent()).toBe(false);
  expect(calls.rename.mock.calls).toEqual([['close-title', 'Keep before closing'], ['close-title', 'Keep before closing']]);
});

it('keeps an acknowledged title on blur while its older metadata is still catching up', async () => {
  show(<TitleField doc={{ id: 'delayed-title', title: 'Original' }} editable onEnter={vi.fn()} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') });
  fireEvent.focus(field);
  fireEvent.change(field, { target: { value: 'Confirmed title' } });
  await waitFor(() => expect(calls.rename).toHaveBeenCalledExactlyOnceWith('delayed-title', 'Confirmed title'), { timeout: 2_000 });
  await waitFor(() => expect(screen.queryByRole('status')).not.toBeInTheDocument());
  fireEvent.blur(field);
  expect(field).toHaveValue('Confirmed title');
});

it('shows a newer teammate title after the pending title is acknowledged', async () => {
  const pending = deferred<unknown>(); calls.rename.mockReturnValueOnce(pending.promise);
  const view = show(<TitleField doc={{ id: 'shared-title', title: 'Original' }} editable onEnter={vi.fn()} />);
  const field = screen.getByRole('textbox', { name: i18n.t('documents.page.titleLabel') });
  fireEvent.change(field, { target: { value: 'My acknowledged title' } });
  await waitFor(() => expect(calls.rename).toHaveBeenCalledTimes(1), { timeout: 2_000 });
  view.rerender(<TitleField doc={{ id: 'shared-title', title: 'Newer teammate title' }} editable onEnter={vi.fn()} />);
  expect(field).toHaveValue('My acknowledged title');
  await act(async () => { pending.resolve(undefined); await pending.promise; });
  expect(field).toHaveValue('Newer teammate title');
  expect(calls.rename).toHaveBeenCalledTimes(1);
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
