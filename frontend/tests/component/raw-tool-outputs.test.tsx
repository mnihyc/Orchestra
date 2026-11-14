/** Retained raw inputs and outputs open through the ordinary lazy file card and viewer. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { RawCall, RawCallView } from '@/features/timeline/RawCall';
import { i18n, setLocale } from '@/i18n';
import { rawOf, toolCall } from '../doubles/thread-rows';
import { show } from './support';

const request = vi.hoisted(() => vi.fn<(path: string, options?: RequestInit) => Promise<Response>>());
vi.mock('@/api/trace', async (original) => ({ ...await original(), traceRequest: request }));
vi.mock('@/features/files/viewers/Code', () => ({ default: () => <div data-viewer="code">Complete tool source</div> }));
const HASH = 'd'.repeat(64);

beforeEach(() => {
  request.mockReset();
  request.mockImplementation(() => Promise.resolve(Response.json({ state: 'ready', hash: HASH, size: 27, media_type: 'text/plain', is_text: true,
    image: null, text: 'Complete tool source', width: null, height: null, duration_seconds: null, entry_count: null })));
});
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('opens the declared whole output without claiming its excerpt is all that was kept (%s)', async (locale) => {
  await setLocale(locale);
  const row = toolCall('native-call', { kind: 'mcp', server: 'gallery', tool: 'render', arguments: {}, result: null, error: null }, { title: 'Native call' });
  if (row.kind !== 'tool_call') throw new Error('The fixture is a tool call');
  const item = { ...row, thread_id: 'retained-thread', payload: { ...row.payload, raw: { ...rawOf(row),
    input: { inline: '', total_len: 27, truncated: true, blob: 'e'.repeat(64) },
    output: { inline: '', total_len: 27, truncated: true, blob: HASH } } } };
  show(<RawCallView item={item} raw={rawOf(item)} />);
  expect(screen.getByRole('button', { name: i18n.t('timeline.raw.fullInput') })).toBeVisible();
  const whole = screen.getByRole('button', { name: i18n.t('timeline.raw.fullOutput') });
  expect(whole).toBeVisible();
  expect(screen.queryByText(i18n.t('timeline.raw.cut'))).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('timeline.raw.copyOutput') })).not.toBeInTheDocument();
  expect(request).not.toHaveBeenCalled();
  await userEvent.click(whole);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('fileMedia.open') }));
  await waitFor(() => expect(document.querySelector('[data-file-overlay] [data-viewer="code"]')).toBeInTheDocument());
  expect(screen.getByRole('dialog')).toHaveAccessibleName(i18n.t('timeline.raw.fullOutput'));
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  expect(whole).toHaveFocus();
});

it('keeps the existing partial-loss notice when no whole output was retained', () => {
  const row = toolCall('partial', { kind: 'other', name: 'future_tool', arguments: null, result: null }, { title: 'Native call' });
  if (row.kind !== 'tool_call') throw new Error('The fixture is a tool call');
  const item = { ...row, payload: { ...row.payload, raw: { ...rawOf(row),
    output: { inline: 'tail', total_len: 1_000, truncated: true, blob: null } } } };
  show(<RawCallView item={item} raw={rawOf(item)} />);
  expect(screen.getByText(i18n.t('timeline.raw.cut'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('timeline.raw.copyOutputShown') })).toBeVisible();
  expect(screen.queryByRole('button', { name: i18n.t('timeline.raw.fullOutput') })).not.toBeInTheDocument();
});

const retained: TimelineContext = { threadId: 'retained-thread', threadName: 'Retained', detail: 'normal', layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING, inspection: { threadId: 'retained-thread' } };

it('reads the call as its harness made it when it is opened, for a row that does not carry it', async () => {
  const row = toolCall('apart', { kind: 'mcp', server: 'gallery', tool: 'render', arguments: {}, result: null, error: null }, { title: 'Native call' });
  if (row.kind !== 'tool_call') throw new Error('The fixture is a tool call');
  const raw = rawOf(row);
  const item = { ...row, thread_id: 'retained-thread', payload: { ...row.payload, raw: undefined } };
  request.mockImplementation(() => Promise.resolve(Response.json(raw)));
  show(<TimelineContextValue.Provider value={retained}><RawCall item={item} /></TimelineContextValue.Provider>);
  expect(await screen.findByText(i18n.t('timeline.raw.tool'))).toBeVisible();
  expect(request).toHaveBeenCalledOnce();
  expect(request.mock.calls[0]?.[0]).toBe('/threads/retained-thread/items/apart/raw?inspect_deleted=true');
  expect(screen.getByText(raw.name, { exact: true })).toBeVisible();
});
