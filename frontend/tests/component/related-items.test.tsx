/** Related reuses one request per document revision and lazy reference cards for every supported kind. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { RelatedPanel } from '@/features/inspector/Related';
import type { ReferencePreview, RelatedItem, RelatedItems } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

const calls = vi.hoisted(() => ({
  related: vi.fn<(id: string, seq: number, cursor?: string) => Promise<RelatedItems>>(),
  preview: vi.fn<(signal: AbortSignal) => Promise<ReferencePreview>>(),
  go: vi.fn(),
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<Record<string, unknown>>(), useRouter: () => ({ history: { push: calls.go } }) }));
vi.mock('@/api/search', async (original) => ({ ...await original<Record<string, unknown>>(), relatedQuery: (id: string, seq: number) => ({ queryKey: ['related-test', id, seq], initialPageParam: null as string | null, queryFn: ({ pageParam }: { pageParam: string | null }) => pageParam === null ? calls.related(id, seq) : calls.related(id, seq, pageParam), getNextPageParam: (page: RelatedItems) => page.next_cursor ?? undefined, staleTime: Infinity, retry: false }) }));
vi.mock('@/api/references', () => ({ referencePreviewQuery: (reference: unknown) => ({ queryKey: ['related-preview-test', reference], queryFn: ({ signal }: { signal: AbortSignal }) => calls.preview(signal), staleTime: Infinity }) }));

const entries: RelatedItem[] = [
  { id: '019a0000-0000-7000-8000-000000000001', kind: 'document', name: 'Rollout checklist', context: [], related_by: ['meaning', 'place'], place: 'topic', references: null },
  { id: '019a0000-0000-7000-8000-000000000002', kind: 'chat', name: 'Release discussion', context: [], related_by: ['references'], place: null, references: 'outgoing' },
  { id: '019a0000-0000-7000-8000-000000000003', kind: 'thread', name: 'Check rollback', context: [], related_by: ['references'], place: null, references: 'incoming' },
];
const reply: RelatedItems = { next_cursor: null, seq: 1, items: entries, meaning: 'ready', meaning_reason: null };
const preview: ReferencePreview = { kind: 'document', id: entries[0]!.id, title: entries[0]!.name, node_kind: null, place: null, description: 'Check traffic before switching', updated_at: null, updated_by: null, part: null };

beforeEach(() => { vi.clearAllMocks(); calls.related.mockResolvedValue(reply); calls.preview.mockResolvedValue(preview); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it('waits for opening, reuses one revision and loads no per-row previews', async () => {
  const view = show(<RelatedPanel documentId="source" seq={1} enabled={false} />);
  expect(calls.related).not.toHaveBeenCalled();
  view.rerender(<RelatedPanel documentId="source" seq={1} />);
  await screen.findByRole('list', { name: 'Related' });
  expect(calls.related).toHaveBeenCalledExactlyOnceWith('source', 1);
  expect(calls.preview).not.toHaveBeenCalled();
  for (const entry of entries) expect(screen.getByRole('link', { name: entry.name })).toBeVisible();
  expect(screen.getByText('Same topic · Similar meaning')).toHaveClass('text-text-muted');
  view.rerender(<RelatedPanel documentId="source" seq={1} enabled={false} />);
  expect(screen.queryByRole('list', { name: i18n.t('related.title') })).not.toBeInTheDocument();
  view.rerender(<RelatedPanel documentId="source" seq={1} />);
  expect(await screen.findByRole('list', { name: i18n.t('related.title') })).toBeVisible();
  expect(calls.related).toHaveBeenCalledTimes(1);
  view.rerender(<RelatedPanel documentId="source" seq={2} />);
  await waitFor(() => expect(calls.related).toHaveBeenCalledTimes(2));
  expect(calls.related).toHaveBeenLastCalledWith('source', 2);
});

it('switches documents without retaining the former document’s rows', async () => {
  let finish: ((value: RelatedItems) => void) | undefined;
  calls.related.mockImplementation((id: string) => id === 'other' ? new Promise<RelatedItems>((resolve) => { finish = resolve; }) : Promise.resolve(reply));
  const view = show(<RelatedPanel documentId="source" seq={1} />);
  await screen.findByRole('link', { name: entries[0]!.name });
  view.rerender(<RelatedPanel documentId="other" seq={1} />);
  expect(screen.queryByRole('link', { name: entries[0]!.name })).not.toBeInTheDocument();
  await waitFor(() => expect(finish).toBeDefined());
  finish?.({ ...reply, items: [] });
  await waitFor(() => expect(screen.queryByRole('list', { name: i18n.t('related.title') })).not.toBeInTheDocument());
});

it('qualifies repeated names with the permitted place and document-folder context', async () => {
  calls.related.mockResolvedValue({ ...reply, items: [
    { ...entries[0]!, name: 'Plan', context: [{ id: 'org', name: 'Work' }, { id: 'folder-a', name: 'Alpha' }] },
    { ...entries[1]!, kind: 'document', name: 'Plan', context: [{ id: 'org', name: 'Work' }, { id: 'folder-b', name: 'Beta' }] },
  ] });
  show(<RelatedPanel documentId="source" seq={1} />);
  expect(await screen.findByRole('link', { name: 'Plan · Alpha' })).toBeVisible();
  expect(screen.getByRole('link', { name: 'Plan · Beta' })).toBeVisible();
  expect(calls.preview).not.toHaveBeenCalled();
});

it('keeps the in-flight preview when a focused card is pinned before its answer arrives', async () => {
  let complete: ((value: ReferencePreview) => void) | undefined;
  let aborted = 0;
  calls.preview.mockImplementation((signal) => new Promise<ReferencePreview>((resolve, reject) => {
    complete = resolve;
    signal.addEventListener('abort', () => { aborted += 1; reject(new DOMException('Aborted', 'AbortError')); }, { once: true });
  }));
  const user = userEvent.setup();
  show(<RelatedPanel documentId="source" seq={1} />);
  await screen.findByRole('link', { name: entries[0]!.name });
  await user.tab();
  await waitFor(() => expect(calls.preview).toHaveBeenCalledTimes(1));
  await user.keyboard('{Enter}');
  await waitFor(() => expect(document.querySelector('[data-reference-card]')).not.toBeNull());
  expect(aborted).toBe(0);
  expect(calls.preview).toHaveBeenCalledTimes(1);
  await act(async () => { complete?.(preview); await Promise.resolve(); });
  expect(await screen.findByRole('button', { name: i18n.t('people.reference.open') })).toBeVisible();
  expect(calls.preview).toHaveBeenCalledTimes(1);
});

it.each(['en', 'zh-CN'] as const)('shows the reference reasons and missing-meaning states in %s', async (locale) => {
  await setLocale(locale);
  for (const meaning of ['off', 'not_ready', 'unavailable'] as const) {
    calls.related.mockResolvedValue({ ...reply, meaning });
    const view = show(<RelatedPanel documentId={meaning} seq={1} />);
    expect(await screen.findByText(i18n.t(`related.meaning.${meaning}`))).toHaveAttribute('role', 'status');
    expect(screen.getByText(i18n.t('related.references.outgoing'))).toBeVisible();
    expect(screen.getByText(i18n.t('related.references.incoming'))).toBeVisible();
    view.unmount();
  }
});

it.each(entries.flatMap((entry) => ['keyboard', 'tap'].map((mode) => ({ entry, mode }))))('opens the existing $entry.kind reference card with $mode', async ({ entry, mode }) => {
  calls.related.mockResolvedValue({ ...reply, items: [entry] });
  calls.preview.mockResolvedValue({ ...preview, id: entry.id, title: entry.name, kind: entry.kind === 'thread' ? 'node' : entry.kind, node_kind: entry.kind === 'thread' ? 'thread' : null });
  const user = userEvent.setup();
  show(<RelatedPanel documentId="source" seq={1} />);
  const chip = await screen.findByRole('link', { name: entry.name });
  expect(calls.preview).not.toHaveBeenCalled();
  if (mode === 'keyboard') { await user.tab(); expect(chip).toHaveFocus(); await user.keyboard('{Enter}'); }
  else await user.click(chip);
  await waitFor(() => expect(document.querySelector('[data-reference-card]')).toHaveTextContent(preview.description));
  expect(calls.preview).toHaveBeenCalledTimes(1);
  expect(calls.go).not.toHaveBeenCalled();
  expect(screen.getByRole('button', { name: i18n.t('people.reference.open') })).toBeVisible();
  await user.keyboard('{Escape}');
  await waitFor(() => expect(document.querySelector('[data-reference-card]')).toBeNull());
});

it('keeps failures local with a retry and leaves an empty list quiet', async () => {
  calls.related.mockRejectedValueOnce(new NetworkError('failed')).mockResolvedValueOnce({ ...reply, items: [] });
  const user = userEvent.setup();
  show(<RelatedPanel documentId="source" seq={1} />);
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.related).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
  expect(screen.queryByRole('list', { name: i18n.t('related.title') })).not.toBeInTheDocument();
});

it('reads only the opened page and asks for the next cursor on demand', async () => {
  calls.related.mockResolvedValueOnce({ ...reply, items: [entries[0]!], next_cursor: 'next-window' })
    .mockResolvedValueOnce({ ...reply, items: [entries[1]!] });
  const user = userEvent.setup();
  show(<RelatedPanel documentId="source" seq={1} />);
  await screen.findByRole('link', { name: entries[0]!.name });
  expect(calls.related).toHaveBeenCalledTimes(1);
  expect(calls.preview).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('related.more') }));
  expect(await screen.findByRole('link', { name: entries[1]!.name })).toBeVisible();
  expect(calls.related).toHaveBeenLastCalledWith('source', 1, 'next-window');
  expect(calls.related).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('button', { name: i18n.t('related.more') })).not.toBeInTheDocument();
  expect(calls.preview).not.toHaveBeenCalled();
});
