/** A person reads a thing by its name, its kind and its time: no hash, identifier, revision counter or build string is a word of a
 * screen, and where support needs one it is a Copy ID act. Each case draws the words of one place that used to show one. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { KeptLayers } from '@/features/resources/hosts/KeptLayers';
import { groupResourceRows, resourceGroupMeta } from '@/features/resources/groups';
import { MachineOrigin } from '@/features/resources/machines/MachineOrigin';
import { RuntimeChange, changeText, releaseOf } from '@/features/resources/TechnicalValue';
import NoPreview from '@/features/files/viewers/NoPreview';
import type { ViewerProps } from '@/features/files/viewers';
import type { HostKeptLayers, MachineRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { CopyId } from '@/ui/CopyId';
import { detailHost } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), Link: ({ to, params, children }: { to: string; params: { id: string }; children: React.ReactNode }) => <a href={to.replace('$id', params.id)}>{children}</a> }));
afterEach(async () => { cleanup(); await setLocale('en'); });

const HOSTNAME = 'fc7220076919';
const container: MachineRef = { id: 'unit-machine', name: 'eu-1', online: true, platform_os: 'linux', placement: 'managed' };
const laptop: MachineRef = { id: 'laptop', name: "Ada's laptop", online: true, platform_os: 'linux', placement: 'connected' };

for (const locale of ['en', 'zh-CN'] as const) {
  it(`says that a resource comes from a container by the host it is on, and from a machine of one's own by its name (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<><p data-testid="container"><MachineOrigin machine={container} /></p><p data-testid="own"><MachineOrigin machine={laptop} /></p></>);
    expect(screen.getByTestId('container')).toHaveTextContent(i18n.t('resourceGroups.fromContainer').replace('<host/>', 'eu-1'));
    expect(within(screen.getByTestId('container')).getByRole('link', { name: 'eu-1' })).toBeVisible();
    expect(screen.getByTestId('own')).toHaveTextContent(`${i18n.t('resourceGroups.from')}${laptop.name}`);
    expect(view.container).not.toHaveTextContent(HOSTNAME);
  });
}

it('groups the machines of one host\'s containers as that host, not once for each container', () => {
  const rows = [
    { id: 'a', origin: { ...container, id: 'unit-a' } },
    { id: 'b', origin: { ...container, id: 'unit-b' } },
    { id: 'c', origin: laptop },
  ];
  const groups = groupResourceRows(rows, 'machine', (row) => resourceGroupMeta({ origin_machine: row.origin }), i18n.getFixedT('en'), 'en');
  expect(groups.map((group) => [group.label, group.rows.map((row) => row.id)])).toEqual([["Ada's laptop", ['c']], ['eu-1', ['a', 'b']]]);
});

it('tells kept layers of an environment apart as the latest and the earlier, not by a number', () => {
  const layer = (generation: number, bytes: number): HostKeptLayers => ({ environment: { id: 'e', name: 'Research' } as HostKeptLayers['environment'], environment_id: 'e', generation, bytes });
  const view = show(<KeptLayers host={{ ...detailHost, kept_layers: [layer(2, 100), layer(7, 300), layer(3, 200)] }} />);
  expect(screen.getAllByText(i18n.t('keptLayers.latest'))).toHaveLength(1);
  expect(screen.getAllByText(i18n.t('keptLayers.earlier'))).toHaveLength(2);
  expect(view.container).not.toHaveTextContent(/generation/i);
});

it('names no hash on a file that has no preview and keeps it behind Copy ID', async () => {
  const user = userEvent.setup(), hash = 'b'.repeat(64);
  const head = { brought: { state: 'ready', hash, size: 10 }, stat: { name: 'data.bin', size: 10, media_type: 'application/octet-stream', hash } };
  const props = { head, openAs: vi.fn() } as unknown as ViewerProps;
  const view = show(<NoPreview {...props} />);
  expect(view.container).not.toHaveTextContent(hash);
  await user.click(screen.getByRole('button', { name: i18n.t('common.copyId') }));
  expect(await navigator.clipboard.readText()).toBe(hash);
});

it('puts an identifier on the clipboard without showing it', async () => {
  const user = userEvent.setup();
  const view = show(<CopyId value="3f2a9c1e-0000-4000-8000-000000000000" />);
  expect(view.container).not.toHaveTextContent('3f2a9c1e');
  await user.click(screen.getByRole('button', { name: i18n.t('common.copyId') }));
  expect(await navigator.clipboard.readText()).toBe('3f2a9c1e-0000-4000-8000-000000000000');
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`says what a unit is behind in by the release a person knows, never by a build string (${locale})`, async () => {
    await setLocale(locale);
    expect(releaseOf('0.1.3-a789093c891f')).toBe('0.1.3');
    expect(releaseOf('0.1.3')).toBe('0.1.3');
    expect(releaseOf('2.1.309')).toBe('2.1.309');
    const newer = show(<RuntimeChange change={{ what: 'connector', name: 'orch-connector', from: '0.1.2-a789093c891f', to: '0.1.3-b123456789ab' }} />);
    expect(newer.container).toHaveTextContent(i18n.t('providers.runtime.update', { name: i18n.t('providers.runtime.tools.connector'), from: '0.1.2', to: '0.1.3' }));
    expect(newer.container).not.toHaveTextContent(/a789093c891f|b123456789ab/);
    newer.unmount();
    const rebuilt = show(<RuntimeChange change={{ what: 'executor', name: 'orch-exec', from: '0.1.3-a789093c891f', to: '0.1.3-b123456789ab' }} />);
    expect(rebuilt.container).toHaveTextContent(i18n.t('providers.runtime.toolRebuilt', { name: i18n.t('providers.runtime.tools.executor'), release: '0.1.3' }));
    expect(rebuilt.container).not.toHaveTextContent(/a789093c891f|b123456789ab/);
    rebuilt.unmount();
    expect(changeText({ what: 'connector', name: 'orch-connector', from: null, to: '0.1.3-b123456789ab' }, i18n.getFixedT(locale))).toBe(i18n.t('providers.runtime.toolAdded', { name: i18n.t('providers.runtime.tools.connector'), release: '0.1.3' }));
  });

  it(`keeps the checksums of a changed definition behind Copy ID, in the details of no sentence (${locale})`, async () => {
    await setLocale(locale);
    const user = userEvent.setup(), from = 'a'.repeat(64), to = 'b'.repeat(64);
    const view = show(<RuntimeChange change={{ what: 'template', name: 'Research template', from, to }} />);
    expect(view.container).toHaveTextContent(i18n.t('resourceDetails.namedChanged', { name: 'Research template' }));
    expect(view.container).not.toHaveTextContent(from);
    expect(screen.queryByText(i18n.t('common.details'), { exact: true })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.copyId') }));
    expect(await navigator.clipboard.readText()).toBe(`${from}\n${to}`);
  });
}
