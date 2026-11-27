/** The server's effective policy and source stay visible while an ineffective own choice remains editable. */
import { useQuery } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ApiError } from '@/api/client';
import { clearSetting, putSetting, registryQuery } from '@/api/settings';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import { settingChoice } from '@/i18n/choices';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { id } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ definitions: [] as SettingDef[], read: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, GET: (path: string) => {
    calls.read(path);
    if (path === '/settings/registry') return Promise.resolve({ data: calls.definitions, response: new Response() });
    return Promise.reject(new Error(`Unexpected policy metadata read ${path}`));
  } } };
});
vi.mock('@/api/settings', async (load) => ({ ...await load<object>(), putSetting: vi.fn(), clearSetting: vi.fn() }));
vi.mock('@/features/navigation/placeLabels', async (loadOriginal) => ({ ...await loadOriginal<object>(), useKnownPlaces: () => new Map() }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const HERE = id(121, 1), target = { kind: 'node' as const, id: HERE };
const local = { scope: 'org' as const, node: { id: HERE, kind: 'org' as const, name: 'Department', redacted: false } };
const upper = { scope: 'system' as const, node: null };
const place = { name: 'Department', kind: 'org', owner: null, access: { permissions: ['settings.manage'] } };
function definition(key: string, merge: SettingDef['merge'], schema: SettingDef['schema'], display?: SettingDef['display']) {
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
  it(`names the inherited ceiling even when this place has already set less ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'broadcast.thread_messages_per_hour';
    definition(key, 'min', { type: 'integer', minimum: 0 });
    const row = setting(key, 5, 5);
    row.source = local;
    row.inherited = { value: 10, source: upper };
    show(<NativeRow value={row} />);
    const ceiling = await screen.findByText(locale === 'en' ? 'Maximum: 10' : '上限：10');
    expect(ceiling.parentElement).toHaveTextContent(i18n.t('settings.editor.from.system'));
  });

  it(`keeps a draft the server finds wider after refusal and saves the corrected limit at the same revision ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'thread.side_max', title = definition(key, 'min', { type: 'integer', minimum: 1 });
    const original = setting(key, 4, 8);
    original.inherited = { value: 4, source: upper };
    const accepted = { ...setting(key, 3, 3, 8), source: local, inherited: original.inherited };
    vi.mocked(putSetting).mockRejectedValueOnce(new ApiError({
      type: 'orch:error:request.invalid', code: 'request.invalid', status: 400, title: 'Invalid request',
      detail: 'Something in this request is not valid. Check what you entered and try again.', request_id: 'test-limit',
      ceiling: { value: 3, source: { scope: 'org', node: { ...local.node, name: 'Research.Team' } } },
      errors: [{ path: 'value', message: 'exceeds the inherited maximum' }],
    }, null)).mockResolvedValueOnce(accepted);
    show(<NativeRow value={original} />);
    const user = userEvent.setup();
    await screen.findByText(i18n.t('settings.compact.aboveStricter'));
    const edit = () => screen.getByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) });
    expect(edit()).toHaveAccessibleDescription('4');
    await user.click(edit());
    const input = screen.getByLabelText(title); expect(input).toHaveDisplayValue('8');
    // Within the maximum the page knows, so it is sent; the place above has lowered it since.
    await user.clear(input); await user.type(input, '4');
    await act(async () => { fireEvent.keyDown(input, { key: 'Enter', isComposing: true }); await Promise.resolve(); });
    expect(putSetting).not.toHaveBeenCalled();
    await user.keyboard('{Enter}');
    await screen.findByText(locale === 'en' ? 'Enter 3 or less (Research.Team).'
      : '请输入 3 或更小的值（Research.Team）。');
    expect(putSetting).toHaveBeenNthCalledWith(1, target, key, { value: 4, expectedRevision: 7 });
    expect(input).toHaveDisplayValue('4');
    expect(screen.queryByText(i18n.t('settings.compact.saved'))).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
    await user.clear(input); await user.type(input, '3'); await user.keyboard('{Enter}');
    await waitFor(() => expect(putSetting).toHaveBeenNthCalledWith(2, target, key, { value: 3, expectedRevision: 7 }));
    await screen.findByText(i18n.t('settings.compact.saved'), { exact: true });
    expect(edit()).toHaveAccessibleDescription('3');
  });

  it(`says a limit above the known maximum once, before sending, with nothing to retry ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'broadcast.thread_messages_per_hour', title = definition(key, 'min', { type: 'integer', minimum: 0 });
    const row = setting(key, 10, null);
    row.own = null; row.parts = [{ source: upper, value: 10, locked: false, ignored: false }];
    row.inherited = { value: 10, source: upper };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('radio', { name: i18n.t('settings.compact.here') }));
    const input = await screen.findByLabelText(title);
    await user.clear(input); await user.type(input, '30'); await user.keyboard('{Enter}');
    const refusal = locale === 'en' ? 'Enter 10 or less (System).' : `请输入 10 或更小的值（${i18n.t('settings.editor.from.system')}）。`;
    expect(await screen.findAllByText(refusal)).toHaveLength(1);
    expect(putSetting).not.toHaveBeenCalled();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).not.toBeInTheDocument();
    expect(input).toHaveDisplayValue('30');
  });

  it(`offers no On where a place above switched a permission off ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'orch_tools.owner_reads';
    definition(key, 'all', { type: 'boolean' });
    const project = { scope: 'project' as const, node: { id: id(121, 2), kind: 'project' as const, name: 'Web', redacted: false } };
    const row: ResolvedSetting = { key, value: false, own: null, source: project, locked_by: null, writable: true,
      parts: [{ source: project, value: false, locked: false, ignored: false }], inherited: { value: false, source: project } };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('combobox'));
    const on = await screen.findByRole('option', { name: new RegExp(`^${i18n.t('settings.editor.words.on')}`) });
    expect(on).toHaveAttribute('aria-disabled', 'true');
    expect(on).toHaveTextContent(i18n.t('settings.compact.heldAbove', { value: i18n.t('settings.editor.words.off'), place: 'Web' }));
    await user.click(on);
    expect(putSetting).not.toHaveBeenCalled();
    expect(screen.getByRole('option', { name: new RegExp(`^${i18n.t('settings.editor.words.off')}`) })).not.toHaveAttribute('aria-disabled', 'true');
  });

  it(`offers no Off where a place above keeps the last output of shells ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'terminal.record';
    definition(key, 'any', { type: 'boolean' });
    const project = { scope: 'project' as const, node: { id: id(121, 2), kind: 'project' as const, name: 'Web', redacted: false } };
    const row: ResolvedSetting = { key, value: true, own: null, source: project, locked_by: null, writable: true,
      parts: [{ source: project, value: true, locked: false, ignored: false }], inherited: { value: true, source: project } };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('combobox'));
    const off = await screen.findByRole('option', { name: new RegExp(`^${i18n.t('settings.editor.words.off')}`) });
    expect(off).toHaveAttribute('aria-disabled', 'true');
    expect(off).toHaveTextContent(i18n.t('settings.compact.heldAbove', { value: i18n.t('settings.editor.words.on'), place: 'Web' }));
    await user.click(off);
    expect(putSetting).not.toHaveBeenCalled();
  });

  it(`offers no looser choice where a place above holds a stricter one, and names the place ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'thread.plan_answered_by';
    definition(key, 'stricter', { type: 'string', enum: ['writers', 'managers'] });
    const project = { scope: 'project' as const, node: { id: id(121, 2), kind: 'project' as const, name: 'Web', redacted: false } };
    const row: ResolvedSetting = { key, value: 'managers', own: null, source: project, locked_by: null, writable: true,
      parts: [{ source: project, value: 'managers', locked: false, ignored: false }], inherited: { value: 'managers', source: project } };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('combobox'));
    const name = (value: string) => settingChoice(i18n.t, key, value)?.name ?? value;
    const writers = await screen.findByRole('option', { name: new RegExp(`^${name('writers')}`) });
    expect(writers).toHaveAttribute('aria-disabled', 'true');
    expect(writers).toHaveTextContent(i18n.t('settings.compact.heldAbove', { value: name('managers'), place: 'Web' }));
    await user.click(writers);
    expect(putSetting).not.toHaveBeenCalled();
    expect(screen.getByRole('option', { name: new RegExp(`^${name('managers')}`) })).not.toHaveAttribute('aria-disabled', 'true');
  });

  it(`says on a policy that the nearest place decides and that a lock above holds it ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'retention.archive_idle_days';
    definition(key, 'replace', { type: 'integer', minimum: 0 });
    const row = setting(key, 30, 30); row.source = local; row.parts = [{ source: local, value: 30, locked: false, ignored: false }];
    show(<NativeRow value={row} />);
    expect(await screen.findByText(i18n.t('settings.compact.nearestDecides'))).toBeVisible();
    cleanup();
    const limit = 'broadcast.thread_messages_per_hour';
    definition(limit, 'min', { type: 'integer', minimum: 0 });
    const bounded = setting(limit, 5, 5); bounded.source = local; bounded.parts = [{ source: local, value: 5, locked: false, ignored: false }];
    show(<NativeRow value={bounded} />);
    await screen.findByText(i18n.t(`day100.builtin.${limit}.title`));
    expect(screen.queryByText(i18n.t('settings.compact.nearestDecides'))).not.toBeInTheDocument();
  });

  it(`offers no wider rule for a runtime a place above forbids and names the place ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'environments.runtimes';
    const title = definition(key, 'map_stricter', { type: 'object', additionalProperties: { type: 'string', enum: ['allow', 'require', 'forbid'] } });
    const above = { '*': 'forbid', runc: 'allow' }, own = { kata: 'allow' };
    const row: ResolvedSetting = { key, value: { ...above, kata: 'forbid' }, own: { value: own, revision: 7, locked: false }, source: upper, locked_by: null, writable: true,
      parts: [{ source: upper, value: above, locked: false, ignored: false }, { source: local, value: own, locked: false, ignored: false }], inherited: { value: above, source: upper } };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) }));
    await user.click(await screen.findByRole('combobox', { name: i18n.t('settings.editor.entry', { key: title, name: 'kata' }) }));
    const forbid = i18n.t('choiceCatalog.runtimePolicy.forbid.name');
    const reason = i18n.t('settings.compact.heldAbove', { value: forbid, place: i18n.t('settings.editor.from.system') });
    for (const wider of ['allow', 'require']) {
      const option = await screen.findByRole('option', { name: new RegExp(`^${i18n.t(`choiceCatalog.runtimePolicy.${wider}.name`)}`) });
      expect(option).toHaveAttribute('aria-disabled', 'true');
      expect(option).toHaveTextContent(reason);
    }
    expect(screen.getByRole('option', { name: new RegExp(`^${forbid}`) })).not.toHaveAttribute('aria-disabled', 'true');
    expect(putSetting).not.toHaveBeenCalled();
  });

  it(`offers no looser decision or lower approver for a capability a place above holds ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    const key = 'capability.policy';
    const title = definition(key, 'map_stricter', { type: 'object', additionalProperties: { type: 'object', additionalProperties: false, required: ['decision', 'approver_scope'],
      properties: { decision: { type: 'string', enum: ['allow', 'ask', 'deny'] }, approver_scope: { type: 'string', enum: ['environment', 'host', 'org', 'system'] } } } });
    const above = { kvm: { decision: 'deny', approver_scope: 'system' } }, own = { kvm: { decision: 'allow', approver_scope: 'environment' } };
    const row: ResolvedSetting = { key, value: above, own: { value: own, revision: 7, locked: false }, source: upper, locked_by: null, writable: true,
      parts: [{ source: upper, value: above, locked: false, ignored: false }, { source: local, value: own, locked: false, ignored: false }], inherited: { value: above, source: upper } };
    show(<NativeRow value={row} />);
    const user = userEvent.setup();
    await user.click(await screen.findByRole('button', { name: i18n.t('settings.compact.editValue', { name: title }) }));
    const system = i18n.t('settings.editor.from.system');
    const decision = await screen.findByRole('combobox', { name: new RegExp(`kvm.*${i18n.t('settings.editor.words.decision', { defaultValue: 'Decision' })}|${i18n.t('settings.editor.words.decision', { defaultValue: 'Decision' })}.*kvm`, 'iu') });
    await user.click(decision);
    const deny = i18n.t('choiceCatalog.capabilityDecision.deny.name');
    for (const looser of ['allow', 'ask']) {
      const option = await screen.findByRole('option', { name: new RegExp(`^${i18n.t(`choiceCatalog.capabilityDecision.${looser}.name`)}`) });
      expect(option).toHaveAttribute('aria-disabled', 'true');
      expect(option).toHaveTextContent(i18n.t('settings.compact.heldAbove', { value: deny, place: system }));
    }
    expect(putSetting).not.toHaveBeenCalled();
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
