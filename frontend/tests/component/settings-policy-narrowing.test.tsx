/** The server's effective policy and source stay visible while an ineffective own choice remains editable. */
import { useQuery } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { clearSetting, putSetting, registryQuery } from '@/api/settings';
import type { ResolvedSetting } from '@/generated/api-types';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { id } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ definitions: [] as unknown[], read: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: async (path: string) => {
    calls.read(path);
    if (path === '/settings/registry') return { data: calls.definitions, response: new Response() };
    throw new Error(`Unexpected policy metadata read ${path}`);
  } } };
});
vi.mock('@/api/settings', async (load) => ({ ...await load<object>(), putSetting: vi.fn(), clearSetting: vi.fn() }));
vi.mock('@/features/navigation/placeLabels', () => ({ useKnownPlaces: () => new Map() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const HERE = id(121, 1), target = { kind: 'node' as const, id: HERE };
const local = { scope: 'org' as const, node: { id: HERE, kind: 'org' as const, name: 'Department', redacted: false } };
const upper = { scope: 'system' as const, node: null };
const place = { name: 'Department', kind: 'org', owner: null, access: { permissions: ['settings.manage'] } };
function definition(key: string, merge: string, schema: unknown, display?: unknown) {
  const title = i18n.t(`day100.builtin.${key}.title`);
  // The actual registry query supplies the wire declaration, including newly declared merge words.
  calls.definitions = [{ key, merge, schema, title, description: '', class: 'policy', scopes: ['system', 'org'],
    group: 'limits', default: null, example: null, text_key: null, keywords: [], ...(display === undefined ? {} : { display }) }];
  return title;
}
function setting(key: string, effective: unknown, own: unknown, revision = 7): ResolvedSetting {
  return { key, value: effective, own: { value: own, revision, locked: false }, source: upper, locked_by: null,
    writable: true, parts: [
      { source: upper, value: effective, locked: true, ignored: false },
      { source: local, value: own, locked: false, ignored: false },
    ] };
}
function NativeRow({ value }: { value: ResolvedSetting }) {
  const registry = useQuery(registryQuery), def = registry.data?.find((row) => row.key === value.key);
  return <ul>{def !== undefined && <SettingRow def={withGroup(def)} setting={value} target={target} place={place} />}</ul>;
}
const appearance = useAppearance.getState();
afterEach(async () => { cleanup(); vi.resetAllMocks(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`saves a looser choice, keeps the parent's answer, undoes its exact revision and follows a later relaxation ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'thread.side_max', title = definition(key, 'min', { type: 'integer', minimum: 1 });
    const original = setting(key, 4, 8), accepted = setting(key, 4, 12, 8), undone = setting(key, 4, 8, 9);
    vi.mocked(putSetting).mockResolvedValueOnce(accepted).mockResolvedValueOnce(undone);
    const view = show(<NativeRow value={original} />), user = userEvent.setup();
    await screen.findByText(i18n.t('settings.compact.aboveStricter'));
    const edit = () => screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) });
    expect(edit()).toHaveAccessibleDescription('4'); expect(putSetting).not.toHaveBeenCalled();
    await user.click(edit());
    const input = screen.getByLabelText(title); expect(input).toHaveDisplayValue('8');
    await user.clear(input); await user.type(input, '12'); await user.keyboard('{Enter}');
    await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, key, { value: 12, expectedRevision: 7 }));
    await screen.findByText(i18n.t('settings.compact.saved'), { exact: true });
    await waitFor(() => expect(edit()).toHaveAccessibleDescription('4'));
    expect(screen.getByText(i18n.t('settings.compact.aboveStricter'))).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('settings.compact.undoNamed', { name: title }) }));
    await waitFor(() => expect(putSetting).toHaveBeenNthCalledWith(2, target, key, { value: 8, locked: false, expectedRevision: 8 }));
    await screen.findByText(i18n.t('settings.compact.undone'), { exact: true });
    const relaxed = { ...undone, value: 8, source: local, parts: [
      { source: upper, value: 20, locked: true, ignored: false },
      { source: local, value: 8, locked: false, ignored: false },
    ] };
    view.rerender(<NativeRow value={relaxed} />);
    await waitFor(() => expect(screen.queryByText(i18n.t('settings.compact.aboveStricter'))).not.toBeInTheDocument());
    expect(edit()).toHaveAccessibleDescription('8'); expect(putSetting).toHaveBeenCalledTimes(2);
    expect(calls.read).toHaveBeenCalledExactlyOnceWith('/settings/registry');
  });

  for (const [key, merge, schema, effective, own] of [
    ['terminal.enabled', 'all', { type: 'boolean' }, false, true],
    ['redaction.mask_secrets', 'any', { type: 'boolean' }, true, false],
    ['chat.group_mentions', 'stricter', { type: 'string', enum: ['writers', 'managers'] }, 'managers', 'writers'],
    ['ask.retention_min_days', 'max', { type: 'integer', minimum: 1 }, 30, 1],
  ] as const) it(`shows the served ${merge} bound without an inert lock or a readonly edit ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const title = definition(key, merge, schema);
    const row = setting(key, effective, own); row.writable = false; row.own = { value: own, revision: 7, locked: true };
    row.parts = row.parts.map((part) => ({ ...part, locked: true }));
    show(<NativeRow value={row} />);
    expect(await screen.findByText(i18n.t('settings.compact.aboveStricter'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) })).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('settings.editor.lockedHere'))).not.toBeInTheDocument();
    expect(putSetting).not.toHaveBeenCalled(); expect(clearSetting).not.toHaveBeenCalled();
  });

  it(`shows an explicit unbounded goal policy as No limit ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'thread.goal.max_turns', title = definition(key, 'min', { type: 'integer', minimum: 0 }, { unit: 'raw', zero: 'no_limit' });
    const row = setting(key, null, 0); row.source = local;
    row.parts = [{ source: local, value: 0, locked: false, ignored: false }];
    show(<NativeRow value={row} />);
    const edit = await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) });
    expect(edit).toHaveAccessibleDescription(i18n.t('goals.editor.noLimit'));
    expect(screen.queryByText(i18n.t('settings.compact.aboveStricter'))).not.toBeInTheDocument();
    expect(putSetting).not.toHaveBeenCalled();
  });
}
