import type * as AccessApi from '@/api/access';
/** Upload history keeps the actor, names, folder, measured size and outcome together. */
import { screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { UploadHistory } from '@/features/files/UploadHistory';
import type { HistoryLine, HistoryLinePage } from '@/generated/api-types';
import { show } from './support';
import userEvent from '@testing-library/user-event';
import { uploadHistoryQuery } from '@/api/access';
import { i18n, setLocale } from '@/i18n';

const fixture = vi.hoisted(() => ({ read: vi.fn<(cursor: string | null) => Promise<HistoryLinePage>>() }));
vi.mock('@/api/access', async (load) => {
  const actual = await load<typeof AccessApi>();
  return { ...actual, uploadHistoryQuery: vi.fn((environment: string, line: string, cursor: string | null) => ({
    ...actual.uploadHistoryQuery(environment, line, cursor), queryFn: () => fixture.read(cursor),
  })) };
});

vi.mock('@/features/people/NamedChip', () => ({ TypedChip: ({ reference }: { reference: { path: string } }) => <a href="#file">{reference.path.split('/').at(-1)}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <button>{user.display_name}</button> }));

const line = {
  id: 'history', at: '2026-01-01T00:00:00Z', actor: { display_name: 'Bo' }, action: 'workspace.file_written', text: '', about: null,
  upload: { environment_id: 'workspace', folder: 'src/assets', bytes: 24, replaced: 1, outcome: 'complete', files: ['screenshot.png', 'data.csv', 'notes.txt'].map((name) => ({ name, path: `src/assets/${name}`, size: 8, received: 8, outcome: 'complete', replaced: false })) },
} as HistoryLine;

it('shows the successful batch with file chips and a compact remainder', () => {
  show(<UploadHistory line={line} />);
  expect(screen.getByRole('button', { name: 'Bo' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'screenshot.png' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'data.csv' })).toBeInTheDocument();
  expect(screen.getByText('and 1 more')).toBeInTheDocument();
  expect(screen.getByText('to src/assets')).toBeInTheDocument();
  expect(screen.getByText('· 24 B')).toBeInTheDocument();
  expect(screen.getByText('· replaced 1')).toBeInTheDocument();
});

it.each(['failed', 'cancelled'] as const)('says %s with a measured partial size and no link to an unwritten file', (outcome) => {
  show(<UploadHistory line={{ ...line, upload: { ...line.upload!, outcome, replaced: 0, bytes: 7, files: [{ name: 'partial.txt', path: null, size: 100, received: 7, outcome, replaced: false }] } }} />);
  expect(screen.getByText(outcome === 'failed' ? 'upload failed' : 'upload cancelled')).toBeInTheDocument();
  expect(screen.getByText('partial.txt')).toBeInTheDocument();
  expect(screen.queryByRole('link')).not.toBeInTheDocument();
  expect(screen.getByText('· 7 B')).toBeInTheDocument();
});


afterEach(async () => { delete document.documentElement.dataset.theme; await setLocale('en'); vi.restoreAllMocks(); vi.clearAllMocks(); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) it(`loads one real file page at a time from a bounded summary (${locale}, ${theme})`, async () => {
  await setLocale(locale); document.documentElement.dataset.theme=theme;
  const t=i18n.getFixedT(locale);
  // The real panel is capped at 18rem. jsdom has no layout; give the real virtualizer its viewport.
  const originalHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight');
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
    if (this.getAttribute('role') === 'log') return 18 * 16;
    const height: unknown = originalHeight?.get?.call(this);
    return typeof height === 'number' ? height : 0;
  });
  const files=line.upload!.files;
  fixture.read.mockImplementation((cursor) => Promise.resolve({items:[{...line,upload:{...line.upload!,files:cursor===null?files.slice(0,2):files.slice(2)}}],next_cursor:cursor===null?'next-files':null}));
  const user=userEvent.setup();
  show(<UploadHistory line={{...line,upload:{...line.upload!,files:files.slice(0,2),file_count:1000,files_paged:true}}}/>);
  expect(screen.queryByRole('link',{name:'notes.txt'})).not.toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:t('uploads.history.more',{number:'998'})}));
  await waitFor(()=>expect(uploadHistoryQuery).toHaveBeenCalledWith('workspace','history',null));
  await user.click(await screen.findByRole('button',{name:t('ui.pagination.more')}));
  expect(await screen.findByRole('link',{name:'notes.txt'})).toBeVisible();
  expect(uploadHistoryQuery).toHaveBeenLastCalledWith('workspace','history','next-files');
  await user.click(screen.getByRole('button',{name:t('uploads.history.first')}));
  await waitFor(()=>expect(uploadHistoryQuery).toHaveBeenLastCalledWith('workspace','history',null));
});
