/** The same announcement controls and inherited tag limit are visible on orgs, projects and topics. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { announce } from '@/api/announcements';
import type * as AnnouncementsApi from '@/api/announcements';
import { PlaceAnnouncements } from '@/features/announcements/Composer';
import { TagPicker } from '@/features/topic/TagPicker';
import type { Announcement, Node, Project, Tag, Topic } from '@/generated/api-types';
import { roleSubject } from '@/api/access';
import { org, me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

vi.mock('@/api/announcements', async (load) => ({ ...await load<typeof AnnouncementsApi>(), announce: vi.fn(), announcementsQuery: (id: string) => ({ queryKey: ['announcements', id], initialPageParam: null,
  queryFn: () => Promise.resolve({ items: [], next_cursor: null }), getNextPageParam: () => undefined }) }));
vi.mock('@/api/settings', () => ({ settingsQuery: (id: string) => ({ queryKey: ['settings', id], queryFn: () => Promise.resolve([{ key: 'tags.max_per_place', value: 1 }]) }) }));
vi.mock('@/api/nodes', () => ({ tagsQuery: (id: string) => ({ queryKey: ['tags', id], queryFn: () => Promise.resolve(labels) }),
  nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: () => Promise.resolve(root) }), setNodeTags: vi.fn() }));
vi.mock('@/api/day100', () => ({ createPickerTag: vi.fn() }));

const root = org('org', 'acme', 'Acme', userRef(me(1, 'ada', 'Ada')), { access: { admin_only: [], participating: true, permissions: ['org.view', 'org.interact', 'org.manage'], role: 'admin', role_from: null, administers: true, owns: true, subscription: null, effective_subscription: 'off' } });
const project: Project = { ...root, kind: 'project', id: 'project', name: 'Web', parent_id: root.id, topic_count: 1, project_count: 0,
  path: [...root.path, { id: 'project', kind: 'project', name: 'Web', redacted: false }], access: { ...root.access, permissions: ['project.view', 'project.interact', 'project.manage'] } };
const topic: Topic = { ...root, kind: 'topic', id: 'topic', name: 'Release', parent_id: project.id, status: 'open', done_at: null, thread_display_state: null,
  path: [...project.path, { id: 'topic', kind: 'topic', name: 'Release', redacted: false }], access: { ...root.access, permissions: ['topic.view', 'topic.interact', 'topic.manage'] },
  thread_count: 0, attention: { blocked: 0, running: 0, waiting_approval: 0 }, work_attention: { uncommitted: 0, unpushed: 0, failed: 0 }, environments: [], default_environment_id: null, last_activity_at: null, seq: 0 };
const labels: Tag[] = [{ id: 'first', name: 'Ready', color: 0, node: root.path[0]! }, { id: 'second', name: 'Urgent', color: 1, node: root.path[0]! }];
const delivered: Announcement = { id: 'announcement', topic_id: topic.id, group_id: 'group', title: 'A release', body: 'Ship it', category: 'notice', severity: 'info',
  from: { user: root.owner }, targets: 'all', files: [], delivery: 'steer', fallback: 'next_turn', wake_idle: false,
  delivered: { interrupted: 0, steered: 0, queued: 1, deferred: 0 }, created_at: '2026-10-15T05:00:00.000Z', topic_seq: 1 };

beforeEach(() => { vi.clearAllMocks(); vi.mocked(announce).mockResolvedValue({ group_id: 'group', announcements: [delivered], topics_skipped: 2 }); });

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
  expect(announce).toHaveBeenCalledWith(place.id, { title: 'A release', body: 'Ship it', category: 'notice', severity: 'info', targets: 'all' });
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
