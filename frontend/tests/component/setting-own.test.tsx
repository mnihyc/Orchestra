/** Saving, resetting and locking a joining setting submit only the local part, preserving failed drafts. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { clearSetting, putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import { TooltipProvider } from '@/ui/Tooltip';
import { show } from './support';
import type { ResolvedSetting as SettingWithParts } from '@/generated/api-types';
import { i18n } from '@/i18n';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), putSetting: vi.fn(), clearSetting: vi.fn() }));
vi.mock('@/api/nodes', async (original) => ({ ...await original<object>(), nodeQuery: (id: string) => ({ queryKey: ['node', id], queryFn: () => Promise.resolve({ kind: 'org', id, address: id, name: id }) }) }));

const def: KeyDef = { key: 'agent.instructions', title: 'Instructions', description: 'Standing instructions', group: 'agent', class: 'parameter', scopes: ['org', 'project', 'topic'], merge: 'append', default: '', example: 'Explain the cause before changing files.', schema: { type: 'string', maxLength: 20000 }, text_key: null, keywords: [] };
const parent = { id: 'team', kind: 'org', name: 'Team', redacted: false } as const;
const here = { id: 'web', kind: 'project', name: 'Web', redacted: false } as const;
const setting: SettingWithParts = { key: def.key, value: 'Ancestor\n\nLocal', own: { value: 'Local', revision: 3, locked: false }, source: { scope: 'project', node: here }, locked_by: null, writable: true, parts: [{ source: { scope: 'org', node: parent }, value: 'Ancestor', locked: false, ignored: false }, { source: { scope: 'project', node: here }, value: 'Local', locked: false, ignored: false }] };
const target = { kind: 'node', id: 'web' } as const;

function commit(field: HTMLElement) {
  const form = field.closest('form');
  if (form === null) throw new Error('The setting has no committing form');
  fireEvent.submit(form);
}

async function draw(row = setting) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = show(<QueryClientProvider client={client}><ul><SettingRow def={def} setting={row} target={target} place={{ name: 'Web', kind: 'project', access: { permissions: ['project.manage', 'settings.manage'] }, owner: null }} onChanged={vi.fn()} /></ul></QueryClientProvider>);
  expect(screen.queryByRole('textbox', { name: 'Instructions' })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Edit Instructions' }));
  return view;
}

describe('one own field', () => {
  it('edits only the own contribution in the row and reads long inherited parts from its existing details', async () => {
    const user = userEvent.setup();
    const long = Array.from({ length: 12 }, (_, index) => `Line ${index + 1}`).join('\n');
    await draw({ ...setting, value: `${long}\n\nLocal`, parts: [
      { source: { scope: 'org', node: parent }, value: long, locked: false, ignored: false },
      { source: { scope: 'project', node: here }, value: 'Local', locked: false, ignored: false },
    ] });
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Local');
    expect(screen.getByRole('textbox', { name: 'Instructions' }).closest('[data-setting-inline-editor]')).not.toBeNull();
    expect(screen.queryByText('Own value', { exact: true })).not.toBeInTheDocument();
    expect(screen.queryByText('Resolved value', { exact: true })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'More about Instructions' }));
    await user.click(screen.getByRole('menuitem', { name: 'Show details' }));
    const inherited = within(screen.getByRole('region', { name: i18n.t('help.hint.where') }));
    expect(inherited.queryByText(/Line 12/)).not.toBeInTheDocument();
    const all = inherited.getByRole('button', { name: 'Show all' });
    expect(all).toHaveAttribute('aria-expanded', 'false');
    await user.click(all);
    expect(inherited.getByText(/Line 12/)).toBeVisible();
    expect(inherited.getByRole('button', { name: 'Show less' })).toHaveAttribute('aria-expanded', 'true');
    expect(screen.queryByRole('group', { name: 'Inherited parts of Instructions' })).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Local');
  });

  beforeEach(() => {
    vi.clearAllMocks();
    // Browser layout measurement is supplied by the real browser cases; jsdom has no observer.
    class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
    vi.stubGlobal('ResizeObserver', Observer);
  });
  afterEach(() => vi.unstubAllGlobals());
  it('keeps ancestor text outside the field and submits only what was edited here', async () => {
    vi.mocked(putSetting).mockResolvedValue(setting);
    await draw();
    const field = screen.getByRole('textbox', { name: 'Instructions' });
    expect(field).toHaveValue('Local');
    expect(field.closest('[data-setting-inline-editor]')).not.toBeNull();
    const resolved = setting.value;
    if (typeof resolved !== 'string') throw new Error('The instructions fixture must contain its joined text');
    expect(field).not.toHaveValue(resolved);
    fireEvent.change(field, { target: { value: 'Edited local' } });
    commit(field);
    await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 'Edited local', expectedRevision: 3 }));
  });

  it('locks an own part without submitting the joined result', async () => {
    vi.mocked(putSetting).mockResolvedValue(setting);
    await draw();
    await userEvent.click(screen.getByRole('button', { name: 'More about Instructions' }));
    await userEvent.click(screen.getByRole('menuitemcheckbox', { name: 'Lock for places below' }));
    await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 'Local', locked: true, expectedRevision: 3 }));
  });

  it('native input keeps the typed text before an ordinary textarea selection or arrow key', async () => {
    await draw();
    const field = screen.getByRole('textbox', { name: 'Instructions' });
    fireEvent.input(field, { target: { value: 'Typed by the browser\nSecond line' } });
    fireEvent.keyDown(field, { key: 'a', ctrlKey: true });
    fireEvent.keyDown(field, { key: 'ArrowRight' });
    await waitFor(() => expect(field).toHaveValue('Typed by the browser\nSecond line'));
  });

  it('keeps a confirmed own value while the parent query still holds the preceding row', async () => {
    vi.mocked(putSetting).mockResolvedValue({ ...setting, own: { value: 'Confirmed local', revision: 4, locked: false }, value: 'Ancestor\n\nConfirmed local' });
    await draw();
    fireEvent.change(screen.getByRole('textbox', { name: 'Instructions' }), { target: { value: 'Confirmed local' } });
    commit(screen.getByRole('textbox', { name: 'Instructions' }));
    await waitFor(() => expect(putSetting).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Edit Instructions' })).toHaveAttribute('aria-expanded', 'false'));
    await userEvent.click(screen.getByRole('button', { name: 'Edit Instructions' }));
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Confirmed local');
    fireEvent.change(screen.getByRole('textbox', { name: 'Instructions' }), { target: { value: 'Next local' } });
    commit(screen.getByRole('textbox', { name: 'Instructions' }));
    await waitFor(() => expect(putSetting).toHaveBeenLastCalledWith(target, def.key, { value: 'Next local', expectedRevision: 4 }));
  });

  it('reset removes only this scope’s stored row', async () => {
    vi.mocked(clearSetting).mockResolvedValue({ ...setting, own: null, value: 'Ancestor' });
    await draw();
    await userEvent.click(screen.getByRole('button', { name: 'More about Instructions' }));
    await userEvent.click(screen.getByRole('menuitem', { name: 'Reset to inherited' }));
    await waitFor(() => expect(clearSetting).toHaveBeenCalledWith(target, def.key, 3));
  });

  it('a failed save leaves the proposed local text in its field', async () => {
    vi.mocked(putSetting).mockRejectedValue(new Error('The request failed'));
    await draw();
    fireEvent.change(screen.getByRole('textbox', { name: 'Instructions' }), { target: { value: 'Keep this draft' } });
    commit(screen.getByRole('textbox', { name: 'Instructions' }));
    expect(await screen.findByRole('button', { name: 'Try again' })).not.toBeDisabled();
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Keep this draft');
  });

  it('a newer own value keeps the draft and cannot rebase its guard before explicit review', async () => {
    vi.mocked(putSetting).mockResolvedValue({ ...setting, own: { value: 'Still writing this', revision: 5, locked: false }, value: 'Changed ancestor\n\nStill writing this' });
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    const frame = (value: SettingWithParts) => <TooltipProvider><QueryClientProvider client={client}><ul><SettingRow def={def} setting={value} target={target} place={{ name: 'Web', kind: 'project', access: { permissions: ['project.manage'] }, owner: null }} onChanged={vi.fn()} /></ul></QueryClientProvider></TooltipProvider>;
    const shown = render(frame(setting));
    await userEvent.click(screen.getByRole('button', { name: 'Edit Instructions' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'Instructions' }), { target: { value: 'Still writing this' } });
    shown.rerender(frame({ ...setting, value: 'Changed ancestor\n\nOther writer', own: { value: 'Other writer', revision: 4, locked: false } }));
    expect(screen.getByRole('textbox', { name: 'Instructions' })).toHaveValue('Still writing this');
    commit(screen.getByRole('textbox', { name: 'Instructions' }));
    expect(putSetting).not.toHaveBeenCalled();
    await userEvent.click(screen.getByText('Review changes', { exact: true }));
    expect(within(screen.getByRole('table', { name: 'Review changes' })).getByText('Other writer')).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: 'Keep my changes' }));
    await waitFor(() => expect(putSetting).toHaveBeenCalledWith(target, def.key, { value: 'Still writing this', expectedRevision: 4 }));
  });
});
