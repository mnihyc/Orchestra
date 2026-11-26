/** The same announcement controls and inherited tag limit are visible on orgs, projects and topics. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { setNodeTags } from '@/api/nodes';
import type * as RecentsApi from '@/api/pickerRecents';
import type { PageOf } from '@/api/collection';
import { announce } from '@/api/announcements';
import type * as AnnouncementsApi from '@/api/announcements';
import { PlaceAnnouncements } from '@/features/announcements/Composer';
import { TagPicker } from '@/features/topic/TagPicker';
import type { Announcement, Node, Project, Tag, Topic } from '@/generated/api-types';
import { roleSubject } from '@/api/access';
import { i18n } from '@/i18n';
import { org, me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

vi.mock('@/api/announcements', async (load) => ({ ...await load<typeof AnnouncementsApi>(), announce: vi.fn(), announcementsQuery: (id: string) => ({ queryKey: ['announcements', id], initialPageParam: null,
  queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }) }));
const settingsState = vi.hoisted(() => ({ wake: true, tags: 1 }));
const tagReads = vi.hoisted(() => ({ scope: vi.fn<(id: string) => void>(), page: vi.fn<(id: string, q: string, cursor: string | null) => Promise<PageOf<Tag>>>(), recent: [] as string[] }));
vi.mock('@/api/settings', () => ({ settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve([{ key: 'tags.max_per_place', value: settingsState.tags }, { key: 'broadcast.wake_idle', value: settingsState.wake }]) }) }));
vi.mock('@/api/nodes',async(loadOriginal)=>({...await loadOriginal<object>(), tagsPageQuery: (id: string, q: string) => ({ queryKey: ['tags', id, 'pages', q], initialPageParam: null, queryFn: ({ pageParam }: { pageParam: string | null }) => tagReads.page(id, q, pageParam), getNextPageParam: (page: PageOf<Tag>) => page.next_cursor }), tagsQuery: (id: string) => ({ queryKey: ['tags', id], queryFn: () => Promise.resolve(labels) }),
  nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: () => { tagReads.scope(id); return Promise.resolve(scopeNodes.find((node) => node.id === id) ?? root); } }), setNodeTags: vi.fn() }));
vi.mock('@/api/day100', () => ({ createPickerTag: vi.fn() }));

vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof RecentsApi>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => Promise.resolve({ count: 5, choices: { project: tagReads.recent } }) }), recordPickerChoice: () => Promise.resolve({ count: 5, remembered: true }) };
});
const session = client.session.getState();
afterEach(() => { cleanup(); client.session.setState(session); });
const root = org('org', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')), { access: { admin_only: [], participating: true, permissions: ['org.view', 'org.interact', 'org.manage'], role: 'admin', role_from: null, administers: true, owns: true, subscription: null, effective_subscription: 'off' } });
const project: Project = { ...root, kind: 'project', id: 'project', name: 'Web', parent_id: root.id, topic_count: 1, project_count: 0,
  path: [...root.path, { id: 'project', kind: 'project', name: 'Web', redacted: false }], access: { ...root.access, permissions: ['project.view', 'project.interact', 'project.manage'] } };
const topic: Topic = { ...root, kind: 'topic', id: 'topic', name: 'Release', parent_id: project.id, status: 'open', done_at: null, thread_display_state: null,
  path: [...project.path, { id: 'topic', kind: 'topic', name: 'Release', redacted: false }], access: { ...root.access, permissions: ['topic.view', 'topic.interact', 'topic.manage'] },
  thread_count: 0, attention: { blocked: 0, running: 0, waiting_approval: 0 }, work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, environments: [], default_environment_id: null, last_activity_at: null, seq: 0 };
let scopeNodes: Node[] = [root, project, topic];
const labels: Tag[] = [{ id: 'first', name: 'Ready', color: 0, node: root.path[0]! }, { id: 'second', name: 'Urgent', color: 1, node: root.path[0]! }];
const delivered: Announcement = { id: 'announcement', topic_id: topic.id, group_id: 'group', title: 'A release', body: 'Ship it', category: 'notice', severity: 'info',
  from: { user: root.owner }, targets: 'all', files: [], delivery: 'steer', fallback: 'next_turn', wake_idle: false,
  delivered: { interrupted: 0, steered: 0, queued: 1, deferred: 0 }, created_at: '2026-10-15T05:00:00.000Z', topic_seq: 1 };

beforeEach(async () => { await i18n.changeLanguage('en'); settingsState.wake = true; settingsState.tags = 1; scopeNodes = [root, project, topic]; tagReads.recent = []; vi.clearAllMocks(); tagReads.page.mockReset().mockImplementation((_id, q) => Promise.resolve({ items: labels.filter((tag) => tag.name.toLowerCase().includes(q.toLowerCase())), next_cursor: null })); vi.mocked(announce).mockResolvedValue({ group_id: 'group', announcements: [delivered], topics_skipped: 2 }); });

it.each([root, project, topic])('shows and sends the same scoped announcement controls in $kind', async (place) => {
  const user = userEvent.setup();
  show(<PlaceAnnouncements place={place} />);
  await user.click(screen.getByRole('button', { name: 'Announcements' }));
  const dialog = screen.getByRole('dialog', { name: `Announcements in ${place.name}` });
  expect(within(dialog).getByText(/private branches are skipped/)).toBeVisible();
  await user.click(within(dialog).getByRole('textbox', { name: 'Title' })); await user.paste('A release');
  await user.click(within(dialog).getByRole('textbox', { name: 'Message' })); await user.paste('Ship it');
  await user.click(within(dialog).getByRole('button', { name: 'Send announcement' }));
  const sent = await within(dialog).findByText('Sent to 1 topic.');
  expect(sent).toBeVisible();
  // The preview also has a status role; delivery counts belong to the sent result.
  const result = sent.closest('[role="status"]');
  expect(result).toHaveTextContent('2 topics skipped.');
  expect(result).toHaveTextContent('1 queued');
  expect(announce).toHaveBeenCalledWith(place.id, { title: 'A release', body: 'Ship it', category: 'notice', severity: 'info', targets: 'all', wake_idle: false });
});

it.each(['en', 'zh-CN'])('makes wake an unticked per-send choice in %s', async (language) => {
  await i18n.changeLanguage(language);
  const user = userEvent.setup();
  show(<PlaceAnnouncements place={topic} />);
  await user.click(screen.getByRole('button', { name: i18n.t('placeAnnouncements.title') }));
  const wake = screen.getByRole('checkbox', { name: i18n.t('placeAnnouncements.wake') });
  await waitFor(() => expect(wake).toBeEnabled());
  expect(wake).not.toBeChecked();
  await user.click(wake);
  await user.type(screen.getByRole('textbox', { name: i18n.t('placeAnnouncements.heading') }), 'A release');
  await user.click(screen.getByRole('button', { name: i18n.t('placeAnnouncements.send') }));
  await waitFor(() => expect(announce).toHaveBeenCalledWith(topic.id, expect.objectContaining({ wake_idle: true })));
  await waitFor(() => expect(wake).not.toBeChecked());
});

it.each(['en', 'zh-CN'])('explains unavailable wake and still sends without it in %s', async (language) => {
  settingsState.wake = false;
  await i18n.changeLanguage(language);
  const user = userEvent.setup();
  show(<PlaceAnnouncements place={topic} />);
  await user.click(screen.getByRole('button', { name: i18n.t('placeAnnouncements.title') }));
  const wake = screen.getByRole('checkbox', { name: i18n.t('placeAnnouncements.wake') });
  await screen.findByText(i18n.t('placeAnnouncements.wakeUnavailable'));
  expect(wake).toBeDisabled();
  expect(wake).not.toBeChecked();
  expect(wake).toHaveAccessibleDescription(i18n.t('placeAnnouncements.wakeUnavailable'));
  await user.type(screen.getByRole('textbox', { name: i18n.t('placeAnnouncements.heading') }), 'A release');
  await user.click(screen.getByRole('button', { name: i18n.t('placeAnnouncements.send') }));
  await waitFor(() => expect(announce).toHaveBeenCalledWith(topic.id, expect.objectContaining({ wake_idle: false })));
});

it('lets a reader inspect announcements while archived and offers no sending form', async () => {
  show(<PlaceAnnouncements place={{ ...root, archived: true }} />);
  await userEvent.click(screen.getByRole('button', { name: 'Announcements' }));
  expect(await screen.findByText('No announcements here yet.')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Send announcement' })).not.toBeInTheDocument();
});

it('offers familiar announcement terms first and explains their effect beside them', async () => {
  const user = userEvent.setup();
  show(<PlaceAnnouncements place={root} />);
  await user.click(screen.getByRole('button', { name: 'Announcements' }));
  const delivery = screen.getByRole('combobox', { name: 'When a turn is running' });
  expect(delivery).toHaveTextContent('Default');
  await user.click(delivery);
  expect(screen.getByRole('option', { name: /^Steer/ })).toBeVisible();
  expect(screen.getByText('Send into the current turn without stopping it.')).toBeVisible();
  expect(screen.getByRole('option', { name: /^Interrupt/ })).toBeVisible();
  expect(screen.getByText('Stop the current turn and deliver this first.')).toBeVisible();
});

it.each([root, project, topic])('shows assigned tags and the resolved limit in $kind', async (base) => {
  const place: Node = { ...base, tags: [labels[0]!] };
  show(<TagPicker place={place} allowed />);
  expect(screen.getByText('Ready')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Edit tags' }));
  expect(await screen.findByText('A place carries at most 1 tag here.')).toBeVisible();
  expect(screen.getByRole('checkbox', { name: 'Urgent' })).toBeDisabled();
  expect(screen.getByRole('checkbox', { name: 'Ready' })).toBeChecked();
  expect(screen.getByRole('checkbox', { name: 'Ready' })).toHaveAccessibleDescription('Defined in Acme.');
});

it('keeps a reader’s tags visible with no editing controls and keeps the member subject word when choosing a role holder', () => {
  show(<TagPicker place={{ ...root, tags: [labels[0]!] }} allowed={false} />);
  expect(screen.getByText('Ready')).toBeVisible();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(roleSubject({ kind: 'members', id: root.id }, root.id)).toEqual({ kind: 'members', id: root.id });
});

it('reads only the selected definition place and offers recent ancestor places without changing the entered name', async () => {
  const path = [root.path[0]!];
  scopeNodes = [root];
  for (let index = 0; index < 13; index++) {
    const ref = { id: `scope-${index}`, kind: 'project' as const, name: `Scope ${index}`, redacted: false };
    path.push(ref); scopeNodes.push({ ...project, id: ref.id, name: ref.name, path: [...path] });
  }
  const place = scopeNodes.at(-1); if (place === undefined) throw new Error('A native selected place is required');
  client.session.setState({ userId: 'tag-reader', status: 'signed_in', readerVersion: 831 }); tagReads.recent = ['scope-1'];
  const user = userEvent.setup(); show(<TagPicker place={place} allowed />);
  await user.click(screen.getByRole('button', { name: i18n.t('placeTags.add') }));
  await user.type(screen.getByRole('textbox', { name: i18n.t('day100.tags.name') }), 'Fresh label');
  const field = await screen.findByRole('combobox', { name: i18n.t('day100.tags.defineIn') });
  await waitFor(() => expect(tagReads.scope.mock.calls.map(([id]) => id)).toEqual([place.id]));
  await user.click(field); expect(await screen.findByRole('option', { name: 'Scope 1' })).toBeVisible();
  expect(screen.getAllByRole('option')[0]).toHaveAccessibleName('Scope 1');
  await user.click(screen.getByRole('option', { name: 'Scope 7' }));
  await waitFor(() => expect(tagReads.scope.mock.calls.map(([id]) => id)).toEqual([place.id, 'scope-7']));
  expect(screen.getByRole('textbox', { name: i18n.t('day100.tags.name') })).toHaveValue('Fresh label');
  expect(setNodeTags).not.toHaveBeenCalled();
});

it('searches native tag pages while keeping every already assigned tag in its complete change', async () => {
  settingsState.tags = 2;
  const late: Tag = { id: 'late-tag', name: 'Late label', color: 1, node: root.path[0]! };
  tagReads.page.mockImplementation((_id, q, cursor) => Promise.resolve(q === 'l' ? { items: [late], next_cursor: null }
    : cursor === null ? { items: [labels[0]!], next_cursor: 'next' } : { items: [late], next_cursor: null }));
  const place = { ...root, tags: [labels[0]!] }; const user = userEvent.setup(); show(<TagPicker place={place} allowed />);
  await user.click(screen.getByRole('button', { name: i18n.t('placeTags.edit') }));
  expect(await screen.findByRole('checkbox', { name: 'Ready' })).toBeChecked();
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  expect(await screen.findByRole('checkbox', { name: late.name })).toBeVisible();
  await user.type(screen.getByRole('textbox', { name: i18n.t('day100.tags.name') }), 'l');
  await waitFor(() => expect(tagReads.page).toHaveBeenCalledWith(place.id, 'l', null));
  await user.click(await screen.findByRole('checkbox', { name: late.name }));
  await waitFor(() => expect(setNodeTags).toHaveBeenCalledWith(place.id, [labels[0]!.id, late.id], place.revision));
});

it('retires an old tag name and definition scope when the same surface opens another target', async () => {
  const user = userEvent.setup(), shown = show(<TagPicker place={root} allowed />);
  await user.click(screen.getByRole('button', { name: i18n.t('placeTags.add') }));
  await user.type(screen.getByRole('textbox', { name: i18n.t('day100.tags.name') }), 'Old target draft');
  shown.rerender(<TagPicker place={topic} allowed />);
  expect(screen.queryByRole('textbox', { name: i18n.t('day100.tags.name') })).toBeNull();
  await user.click(screen.getByRole('button', { name: i18n.t('placeTags.add') }));
  expect(screen.getByRole('textbox', { name: i18n.t('day100.tags.name') })).toHaveValue('');
  await waitFor(() => expect(tagReads.scope.mock.calls.map(([id]) => id)).toEqual([root.id, topic.id]));
});
