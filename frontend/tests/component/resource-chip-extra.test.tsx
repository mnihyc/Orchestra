/** All resource kinds share the name card, one detail read and keyboard/tap behavior in lists and choices. */
import { act, screen, waitFor, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import type { Skill } from '@/generated/api-types';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import ResourceCard from '@/features/resource-facts/ResourceCard';
import { ResourceSentence } from '@/features/resource-facts/ResourceSentence';
import type { ResourceSubject } from '@/features/resource-facts/facts';
import { Select } from '@/ui/Select';
import { Table } from '@/ui/Table';
import { queryClient } from '@/api/query';
import type { ReactNode } from 'react';
import { detailAccess, detailAgent, detailHome, detailMachineRef, detailMcp, detailMount, detailTemplate } from './resource-detail-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ row: {}, reads: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const original = await load<object>();
  return { ...original, api: { GET: (path: string, options: { params: { path: { id: string } } }) => { server.reads(path); return Promise.resolve({ data: { ...server.row, id: options.params.path.id }, error: undefined, response: new Response(null, { status: 200 }) }); } } };
});
beforeEach(() => server.reads.mockClear());

function display(content: ReactNode) {
  const cache = new QueryClient({ defaultOptions: queryClient.getDefaultOptions() });
  return show(<QueryClientProvider client={cache}>{content}</QueryClientProvider>);
}

const skill: Skill = { id: 'skill', name: 'Review changes', address: 'team.review', description: '', home: detailHome, org_id: 'team', owner: null,
  access: { ...detailAccess, permissions: ['skill.view'] }, enabled: true, versions: 1, draft: null, origin: { kind: 'upload', name: 'review.zip', folder: null }, revision: 1,
  latest: { id: 'version', number: 3, name: 'Review changes', description: '', file_count: 1, size: 20, archive: { blob: 'synthetic-archive', size: 20 }, created_by: null, created_at: '2026-10-25T12:00:00Z', origin: { kind: 'upload', name: 'review.zip', folder: null } } };
const cases: { subject: ResourceSubject; row: Record<string, unknown>; expected: string; path: string }[] = [
  { subject: { kind: 'mount', mount: detailMount }, row: { ...detailMount, attached_environments: 2, locations: [{ machine: detailMachineRef, kind: 'volume', source: 'cache-volume', storage: 'fast', measured_bytes: 0 }] }, expected: 'Attached environments', path: '/resources/mounts/mount' },
  { subject: { kind: 'mcp_server', server: detailMcp }, row: { ...detailMcp, enabled_places: 2 }, expected: 'Transport', path: '/resources/mcp/mcp' },
  { subject: { kind: 'template', template: detailTemplate }, row: { ...detailTemplate, spec: { image: 'example.test/runtime@sha256:abcdef' } }, expected: 'Image', path: '/resources/templates/template' },
  { subject: { kind: 'agent', agent: detailAgent }, row: { ...detailAgent, providers: [{ id: 'provider', name: 'Team models' }], models: [{ provider_id: 'provider', model_name: 'review', display_name: 'Review model', parameters: {} }] }, expected: 'Program', path: '/resources/agents/agent' },
  { subject: { kind: 'skill', skill }, row: { ...skill, enabled_places: 4 }, expected: 'Version', path: '/resources/skills/skill' },
];

for (const entry of cases) it(`opens the ${entry.subject.kind} body by tap with one native detail read`, async () => {
  server.row = entry.row;
  // The card module is loaded before the interaction; the test measures reads, not Vite transforms.
  expect(ResourceCard).toBeDefined();
  const user = userEvent.setup();
  display(<ResourceChip subject={entry.subject} hint={false} />);
  expect(server.reads).not.toHaveBeenCalled();
  const trigger = screen.getByRole('button');
  await user.click(trigger);
  expect(await screen.findByText(entry.expected, { exact: true })).toBeVisible();
  await waitFor(() => expect(server.reads).toHaveBeenCalledTimes(1));
  const body = within(screen.getByRole('dialog'));
  switch (entry.subject.kind) {
    case 'mount':
      expect(body.getByText('cache-volume')).toBeVisible();
      expect(body.getByText('Build machine')).toBeVisible();
      expect(body.getByText('fast')).toBeVisible();
      expect(body.getByText('0 B')).toBeVisible();
      expect(body.getByText('Mount as-is')).toBeVisible();
      expect(body.getByText('2', { exact: true })).toBeVisible();
      break;
    case 'mcp_server':
      expect(body.getByText('stdio')).toBeVisible();
      expect(body.getByText('Machines').closest('dt')?.nextElementSibling).toHaveTextContent('No machine');
      expect(body.getByText('2', { exact: true })).toBeVisible();
      break;
    case 'template':
      expect(body.getByText('example.test/runtime@sha256:abcdef')).toBeVisible();
      expect(body.getByText('0', { exact: true })).toBeVisible();
      break;
    case 'agent':
      expect(body.getByText('Program', { exact: true }).closest('dt')?.nextElementSibling).toHaveTextContent('Codex');
      expect(body.getByRole('button', { name: 'Team models' })).toBeVisible();
      expect(body.getByRole('button', { name: 'Review model' })).toBeVisible();
      break;
    case 'skill':
      expect(body.getByText('3', { exact: true })).toBeVisible();
      expect(body.getByText('Upload · review.zip')).toBeVisible();
      expect(body.getByText('4', { exact: true })).toBeVisible();
  }
  expect(screen.getByRole('heading').querySelector('a')).toHaveAttribute('href', entry.path);
  await user.keyboard('{Escape}');
  expect(trigger).toHaveFocus();
});

for (const entry of [
  { name: 'a separately pinned image', spec: { image: 'example.test/runtime:stable', image_digest: 'sha256:abcdef' }, expected: ['example.test/runtime:stable', 'Digest', 'sha256:abcdef'] },
  { name: 'a Dockerfile source', spec: { build: { dockerfile: 'FROM example.test/runtime:stable' } }, expected: ['Built from a Dockerfile'] },
]) it(`shows ${entry.name} from the native template spec with one read`, async () => {
  server.row = { ...detailTemplate, spec: entry.spec };
  const user = userEvent.setup();
  display(<ResourceChip subject={{ kind: 'template', template: detailTemplate }} hint={false} />);
  expect(server.reads).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: detailTemplate.name }));
  const body = within(screen.getByRole('dialog'));
  for (const text of entry.expected) expect(await body.findByText(text, { exact: true })).toBeVisible();
  expect(server.reads).toHaveBeenCalledTimes(1);
});

it('keeps a localized plan sentence and inspects its mount name without a row read', async () => {
  server.row = { ...detailMount };
  const user = userEvent.setup();
  display(<p><ResourceSentence text="Copy research-data to the destination" resources={[{ label: detailMount.name, subject: { kind: 'mount', mount: detailMount } }]} /></p>);
  expect(screen.getByText(/Copy/).textContent).toBe('Copy research-data to the destination');
  expect(server.reads).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: detailMount.name }));
  expect(await screen.findByText('Default on move')).toBeVisible();
  expect(server.reads).toHaveBeenCalledTimes(1);
});

it('renders many table chips without a detail request until a name is focused', async () => {
  server.row = { ...detailTemplate };
  const rows = Array.from({ length: 20 }, (_, index) => ({ ...detailTemplate, id: String(index), name: `Template ${index}` }));
  display(<Table label="Templates" rows={rows} rowKey={(row) => row.id} columns={[{ id: 'name', header: 'Template', cell: (row) => <ResourceChip subject={{ kind: 'template', template: row }} hint={false} /> }]} />);
  expect(server.reads).not.toHaveBeenCalled();
  act(() => screen.getByRole('button', { name: 'Template 0' }).focus());
  expect(await screen.findByText('Image', { exact: true })).toBeVisible();
  expect(server.reads).toHaveBeenCalledTimes(1);
});

it('inspects a mount in a select without changing the chosen resource', async () => {
  server.row = { ...detailMount };
  const choose = vi.fn(); const user = userEvent.setup();
  display(<Select label="Mount" value="mount" onValueChange={choose} options={[{ value: 'mount', name: detailMount.name, ...resourceChoice({ kind: 'mount', mount: detailMount }) }]} />);
  await user.click(screen.getByRole('combobox', { name: 'Mount' }));
  await user.keyboard('{ArrowRight}');
  expect(await screen.findByText('Locations', { exact: true })).toBeVisible();
  expect(choose).not.toHaveBeenCalled();
});
