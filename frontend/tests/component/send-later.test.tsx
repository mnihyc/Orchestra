/** Human scheduling freezes the full message, preserves failures and uses native revision/receipt boundaries. */
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Schedules from '@/api/inputSchedules';
import type * as Auth from '@/api/auth';
import type * as Library from '@/api/resourceLibrary';
import { ApiError, client } from '@/api/client';
import { ScheduleEditor } from '@/features/schedules/Editor';
import { HumanScheduleActions } from '@/features/schedules/HumanSchedules';
import { inputSchedule, SCHEDULE_THREAD, SCHEDULE_TOPIC } from '../doubles/input-schedules';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import type { ContentPart, CreateThread, InputScheduleCreateBody, InputScheduleWriteResponse } from '@/generated/api-types';
import { me } from '../doubles/fixtures';
import { ADA } from '../doubles/chat-rows';
import { show } from './support';
const calls = vi.hoisted(() => ({ create: vi.fn<typeof Schedules.createInputSchedule>(), update: vi.fn<typeof Schedules.updateInputSchedule>(), read: vi.fn<() => Promise<Schedules.InputSchedule>>(), same: vi.fn<() => boolean>() }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), meQuery: { queryKey: ['send-later-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada', { id: ADA.id })) } }));
vi.mock('@/api/inputSchedules', async (original) => ({ ...await original<typeof Schedules>(), createInputSchedule: calls.create, updateInputSchedule: calls.update, inputScheduleQuery: (id: string) => ({ queryKey: ['input-schedule', id], queryFn: calls.read }) }));
vi.mock('@/api/resourceLibrary', async (original) => ({ ...await original<typeof Library>(), sameLibraryReader: () => calls.same() }));
vi.mock('@/features/timeline/items/UserMessage', () => ({ Part: ({ part }: { part: ContentPart }) => <span>{'text' in part ? part.text : 'name' in part ? part.name : 'Reference'}</span> }));
vi.mock('@/features/start/BindingDialog', () => ({ NewThreadDialog: ({ prepare, onClose }: { prepare: { onChoose: (value: CreateThread) => void }; onClose: () => void }) => <button type="button" onClick={() => { prepare.onChoose({ binding: { provider_id: 'provider', agent_id: 'agent' }, name: 'Prepared' }); onClose(); }}>Apply setup</button> }));
const previous = client.session.getState(), appearance = useAppearance.getState();
const attachment: ContentPart = { type: 'file', name: 'notes.txt', path: '.orch/attachments/notes.txt', blob: 'b'.repeat(64), size: 5 };
const draft = { content: [{ type: 'text', text: 'The whole authored draft' } as const, attachment], nativeAction: { action_id: 'native-choice', revision: 'catalogue-1', arguments: { query: 'status' } } };
const definition = () => inputSchedule({ kind: null, agent: null, author: ADA, definition: { content: draft.content, native_action: draft.nativeAction, target: { kind: 'thread', thread_id: SCHEDULE_THREAD } } });
beforeEach(() => {
  vi.clearAllMocks(); calls.same.mockReturnValue(true); client.session.setState({ status: 'signed_in', userId: ADA.id, readerVersion: 1200 }); useAppearance.setState({ timeZone: 'UTC', locale: 'en' });
  calls.create.mockResolvedValue({ projection: 'receipt', schedule_id: 'made-schedule', revision: 1, topic_seq: 3 });
  calls.update.mockResolvedValue({ projection: 'receipt', schedule_id: 'made-schedule', revision: 2, topic_seq: 4 }); calls.read.mockResolvedValue(definition());
});
afterEach(async () => { cleanup(); client.session.setState(previous); useAppearance.setState(appearance); await setLocale('en'); });
async function once(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByRole('textbox', { name: i18n.t('sendLater.date') }), '2030-02-01');
  const time = document.querySelector<HTMLInputElement>('input[type=time]'); if (time === null) throw new Error('Missing time input');
  fireEvent.change(time, { target: { value: '09:30' } });
}
for (const locale of ['en', 'zh-CN'] as const) {
  it(`opens Once without an invented date/time and keeps the entire draft visible (${locale})`, async () => {
    await setLocale(locale); show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={() => undefined} />);
    expect(screen.getByRole('textbox', { name: i18n.t('sendLater.date') })).toHaveValue(''); expect(document.querySelector('input[type=time]')).toHaveValue('');
    expect(screen.getByText('The whole authored draft')).toBeInTheDocument(); expect(screen.getByText('notes.txt')).toBeInTheDocument(); expect(calls.create).not.toHaveBeenCalled();
  });
  it(`saves only the native schedule and accepts its sparse receipt (${locale})`, async () => {
    await setLocale(locale); const accepted = vi.fn(), close = vi.fn(), user = userEvent.setup();
    show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={close} onAccepted={accepted} />); await once(user);
    await user.click(screen.getByRole('combobox', { name: i18n.t('sendLater.destination') })); await user.click(screen.getByRole('option', { name: i18n.t('sendLater.newThread') }));
    await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
    await waitFor(() => expect(calls.create).toHaveBeenCalledOnce());
    expect(calls.create.mock.calls[0]?.[1]).toEqual({ name: null, content: draft.content, native_action: draft.nativeAction, timing: { kind: 'once', at: '2030-02-01T09:30:00.000Z', zone: 'UTC' }, target: { kind: 'new_thread', config: {} } });
    expect(accepted).toHaveBeenCalledOnce(); expect(close).toHaveBeenCalledOnce(); expect(calls.update).not.toHaveBeenCalled();
  });
  it(`a refused save retains full draft, native selection and chosen timing (${locale})`, async () => {
    await setLocale(locale); calls.create.mockRejectedValue(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: '', status: 403, request_id: 'schedule-save' }, null));
    const accepted = vi.fn(), user = userEvent.setup(); show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={() => undefined} onAccepted={accepted} />); await once(user);
    await user.click(screen.getByRole('combobox', { name: i18n.t('sendLater.destination') })); await user.click(screen.getByRole('option', { name: i18n.t('sendLater.newThread') }));
    await user.click(screen.getByRole('button', { name: i18n.t('common.save') })); await screen.findByRole('alert');
    expect(screen.getByText('The whole authored draft')).toBeInTheDocument(); expect(screen.getByRole('textbox', { name: i18n.t('sendLater.date') })).toHaveValue('2030-02-01'); expect(document.querySelector('input[type=time]')).toHaveValue('09:30'); expect(accepted).not.toHaveBeenCalled();
  });
  it(`only the actual author opens Edit or Resume; another controller may pause (${locale})`, async () => {
    await setLocale(locale); const row = definition(), user = userEvent.setup();
    show(<HumanScheduleActions row={{ ...row, state: 'paused' }} topic={SCHEDULE_TOPIC} controller />);
    expect(screen.getByRole('button', { name: i18n.t('sendLater.edit') })).toBeInTheDocument(); await user.click(screen.getByRole('button', { name: i18n.t('sendLater.resume') }));
    await waitFor(() => expect(calls.update).toHaveBeenCalledWith(row.id, { revision: row.revision, state: 'active', name: null, target: null, content: null, timing: null }, expect.any(String)));
  });
}
it('a lost schedule answer retries the same body/key and does not clear the unconfirmed draft', async () => {
  calls.create.mockRejectedValueOnce(new NetworkError('failed')); const user = userEvent.setup(); show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={() => undefined} />); await once(user);
  await user.click(screen.getByRole('combobox', { name: i18n.t('sendLater.destination') })); await user.click(screen.getByRole('option', { name: i18n.t('sendLater.newThread') }));
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') })); await screen.findByRole('alert'); await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(2)); expect(calls.create.mock.calls[1]).toEqual(calls.create.mock.calls[0]);
});
it('new-thread setup chooses a config without executing its ordinary native creator', async () => {
  const user = userEvent.setup(); show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={() => undefined} />); await once(user);
  await user.click(screen.getByRole('combobox', { name: i18n.t('sendLater.destination') })); await user.click(screen.getByRole('option', { name: i18n.t('sendLater.newThread') }));
  await user.click(screen.getByRole('checkbox', { name: i18n.t('sendLater.topicDefaults') })); await user.click(screen.getByRole('button', { name: i18n.t('sendLater.configure') }));
  await user.click(screen.getByRole('button', { name: 'Apply setup' })); expect(calls.create).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') })); await waitFor(() => expect(calls.create).toHaveBeenCalledOnce());
  const saved: InputScheduleCreateBody | undefined = calls.create.mock.calls[0]?.[1]; expect(saved?.target).toEqual({ kind: 'new_thread', config: { binding: { provider_id: 'provider', agent_id: 'agent' }, name: 'Prepared' } });
});
it('does not clear or close under a replacement reader after a late accepted save', async () => {
  let finish: ((value: InputScheduleWriteResponse) => void) | undefined;
  calls.create.mockImplementation(() => new Promise((resolve) => { finish = resolve; })); const accepted = vi.fn(), close = vi.fn(), user = userEvent.setup();
  show(<ScheduleEditor topic={SCHEDULE_TOPIC} draft={draft} onClose={close} onAccepted={accepted} />); await once(user);
  await user.click(screen.getByRole('combobox', { name: i18n.t('sendLater.destination') })); await user.click(screen.getByRole('option', { name: i18n.t('sendLater.newThread') }));
  await user.click(screen.getByRole('button', { name: i18n.t('common.save') })); await waitFor(() => expect(calls.create).toHaveBeenCalledOnce()); calls.same.mockReturnValue(false);
  await act(async () => { finish?.({ projection: 'receipt', schedule_id: 'made', revision: 1, topic_seq: 1 }); await Promise.resolve(); }); expect(accepted).not.toHaveBeenCalled(); expect(close).not.toHaveBeenCalled();
});
