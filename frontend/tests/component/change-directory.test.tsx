/** A directory remains relative to the native environment and explicit root differs from inheritance. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { Door } from '@/api/workspace';
import { ChangeDirectory } from '@/features/change/Directory';
import { initialChoices, requestOf } from '@/features/change/choices';
import type { useEnvironment } from '@/features/files/door';
import type { Environment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { EMPTY, directoryKey, type DirectoryWindow } from '@/realtime/stores/directoryTypes';
import { RUNS_ON } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { entry } from '../unit/directory-window-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({
  environment: vi.fn<() => ReturnType<typeof useEnvironment>>(),
  directory: vi.fn<(path: string) => DirectoryWindow>(),
  navigate: vi.fn<(path: string, action: string) => void>(),
}));
vi.mock('@/features/files/door', () => ({ useEnvironment: () => reads.environment() }));
vi.mock('@/features/files/useDirectoryTree', () => ({ useDirectoryTree: (door: Door, path: string) => ({
  tree: { navigate: reads.navigate }, capacity: null,
  directories: new Map([[directoryKey(door.environmentId, path), reads.directory(path)]]),
}) }));

let environment: Environment;
beforeEach(() => {
  vi.clearAllMocks();
  const world = new World(), ada = world.people.get('ada');
  if (ada === undefined) throw new Error('The native reader is required');
  const native = world.resources.inventoryView(ada, 'environment', RUNS_ON.workspace);
  if (native?.kind !== 'environment') throw new Error('The native machine environment is required');
  environment = native.value;
  reads.environment.mockReturnValue({ environment, failed: null });
  reads.directory.mockImplementation((path: string): DirectoryWindow => ({ ...EMPTY, status: 'ready', source: 'live',
    entries: path === '' ? [entry('web', '', 'dir'), entry('README.md')] : [],
    canNext: path === '', page: { incoming: null, revision: 'native-observation', ordinal: 1 } }));
});
afterEach(async () => { cleanup(); delete document.documentElement.dataset.theme; await setLocale('en'); });

function Field({ target = true }: { target?: boolean }) {
  const [directory, setDirectory] = useState<string | null>(null);
  return <><ChangeDirectory value={directory} current="" door={target ? { environmentId: environment.id, threadId: null } : null} onChange={setDirectory} />
    <output aria-label="Native change request">{JSON.stringify(requestOf({ ...initialChoices('replace'), directory }))}</output></>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`types and browses machine-relative directories, preserving explicit root and reset ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup(); show(<Field />);
    expect(reads.environment).not.toHaveBeenCalled(); expect(reads.directory).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.change') }));
    await user.type(screen.getByRole('textbox', { name: i18n.t('change.directory.label') }), 'apps/project');
    expect(screen.getByLabelText('Native change request')).toHaveTextContent('"directory":"apps/project"');
    expect(reads.environment).not.toHaveBeenCalled(); expect(reads.directory).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.keep') }));
    expect(screen.getByLabelText('Native change request')).not.toHaveTextContent('directory');
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.change') }));
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.browse') }));
    expect(screen.queryByRole('button', { name: 'README.md' })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('files.pages.next') }));
    expect(reads.navigate).toHaveBeenCalledWith('', 'next');
    await user.click(screen.getByRole('button', { name: 'web' }));
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.use') }));
    expect(screen.getByLabelText('Native change request')).toHaveTextContent('"directory":"web"');
    expect(screen.getByLabelText('Native change request')).not.toHaveTextContent(environment.workspace_root);
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.browse') }));
    await user.click(screen.getByRole('button', { name: i18n.t('files.pages.openParent') }));
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.use') }));
    expect(screen.getByLabelText('Native change request')).toHaveTextContent('"directory":""');
  });

  it(`keeps manual relative entry usable for an environment not created yet ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup(); show(<Field target={false} />);
    await user.click(screen.getByRole('button', { name: i18n.t('change.directory.change') }));
    expect(screen.getByRole('button', { name: i18n.t('change.directory.browse') })).toHaveAttribute('aria-disabled', 'true');
    await user.type(screen.getByRole('textbox', { name: i18n.t('change.directory.label') }), 'src');
    expect(screen.getByLabelText('Native change request')).toHaveTextContent('"directory":"src"');
    expect(reads.environment).not.toHaveBeenCalled();
  });
}

it('does not let an old cached directory be chosen after the environment read fails', async () => {
  reads.environment.mockReturnValue({ environment, failed: new Error('The environment read was refused') });
  const user = userEvent.setup(); show(<Field />);
  await user.click(screen.getByRole('button', { name: i18n.t('change.directory.change') }));
  await user.click(screen.getByRole('button', { name: i18n.t('change.directory.browse') }));
  expect(screen.getByRole('button', { name: i18n.t('change.directory.use') })).toHaveAttribute('aria-disabled', 'true');
  expect(screen.queryByRole('button', { name: 'web' })).not.toBeInTheDocument();
  expect(screen.getByLabelText('Native change request')).not.toHaveTextContent('directory');
});
