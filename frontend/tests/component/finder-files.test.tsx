/** Finder filename hits keep the shared compact file chip and open its ordinary viewer by keyboard or tap. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Sources from '@/features/finder/sources';
import type * as Search from '@/api/search';
import { Finder } from '@/features/finder/Finder';
import { openFinder, useFinder } from '@/features/finder/store';
import { SIGNS } from '@/features/finder/modes';
import type { SearchHit } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { useWorkbench } from '@/features/workbench/store';
import { show } from './support';

const calls = vi.hoisted(() => ({ go: vi.fn(), request: vi.fn() }));
const person = { id: 'reader', kind: 'human' as const, username: 'reader', display_name: 'Reader', avatar: { text: 'R', color: 1 } };
const topic = { id: 'topic', kind: 'topic' as const, name: 'Release', redacted: false };
const at = '2026-10-25T00:00:00Z';
const hit: SearchHit = { kind: 'document', matches: [[0, 5]], snippet: 'photo.png', block_id: null, by: null, path: [topic],
  document: { id: 'doc', title: 'Release notes', description: '', description_source: 'auto', description_line: '', node: topic,
    owner: person, owners: 1, created_by: person, page: 'follows', seq: 1, block_count: 1, bytes: 20, files_bytes: 128,
    version: 1, open_discussions: 0, updated_at: at, updated_by: person, discussion_level: 'involved', seen_seq: null, archived: false, deleted: false },
  file: { id: 'file', name: 'photo.png', media_type: 'image/png', size: 128, is_text: false, added_by: null, added_at: at },
};
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => () => undefined }));
vi.mock('@/features/finder/sources', async (original) => ({ ...await original<typeof Sources>(),
  useEnv: (showCard: Sources.AnswerInput['env']['showCard']): Sources.AnswerInput['env'] => ({ t: i18n.t, go: calls.go, orgSlug: 'team', exists: () => true, showCard }),
  useAnswers: () => ({ answers: {}, full: new Set(), partial: false, pending: false, failure: null }),
}));
vi.mock('@/api/search', async (original) => ({ ...await original<typeof Search>(), searchPreviewQuery: () => ({ queryKey: ['file-finder-hits'], queryFn: () => Promise.resolve({ items: [hit], next_cursor: null }) }) }));
vi.mock('@/api/trace', async (original) => ({ ...await original(), traceRequest: calls.request }));
vi.mock('@/features/files/viewers/Image', () => ({ default: () => <span data-viewer="image">Image viewer</span> }));
const before = useFinder.getState();
beforeEach(() => { vi.clearAllMocks(); useFinder.setState(before); useWorkbench.setState({ chipSizes: {} });
  calls.request.mockImplementation(() => Promise.resolve(Response.json({ state: 'ready', hash: 'a'.repeat(64), size: 128, media_type: 'image/png', is_text: false,
    image: 'data:image/png;base64,cGl4ZWxz', text: null, width: 1, height: 1, duration_seconds: null, entry_count: null }))); });
afterEach(() => { cleanup(); useFinder.setState(before); });

for (const action of ['tap', 'keyboard'] as const) it(`opens a filename result in the shared viewer by ${action}`, async () => {
  const user = userEvent.setup();
  openFinder(`${SIGNS.conversations} photo`);
  show(<Finder ready={false} />);
  const finder = await screen.findByRole('dialog', { name: i18n.t('finder.title') });
  const file = await within(finder).findByRole('link', { name: 'photo.png' });
  expect(file.closest('[data-chip-size]')).toHaveAttribute('data-chip-size', 'small');
  expect(calls.request).not.toHaveBeenCalled();
  if (action === 'tap') await user.click(file);
  else { await user.click(within(finder).getByRole('combobox')); await user.keyboard('{ArrowDown}{ArrowUp}{Enter}'); }
  await waitFor(() => expect(document.querySelector('[data-file-overlay] [data-viewer="image"]')).toBeInTheDocument());
  expect(screen.getByRole('dialog')).toHaveAccessibleName('photo.png');
  expect(calls.go).not.toHaveBeenCalled();
});
