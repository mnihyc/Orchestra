/** MCP choices preserve own entries and use current native writability and revision facts. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type { SettingWrite } from '@/api/settings';
import { McpEnabledSwitch } from '@/features/resources/mcp/EnabledSwitch';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const ID = '00000000-0000-4000-8000-000000000001';
const OTHER = '00000000-0000-4000-8000-000000000002';
const PARENT = '00000000-0000-4000-8000-000000000003';
const server = { id: ID, name: 'docs', enabled: true };
const setting = (more: Partial<ResolvedSetting> = {}): ResolvedSetting => ({
  key: 'mcp.enabled', value: { [ID]: true, [OTHER]: false, [PARENT]: true },
  own: { value: { [OTHER]: false }, revision: 7, locked: false },
  parts: [], source: { scope: 'environment_own', node: null }, locked_by: null, writable: true, ...more,
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('turns off inherited use without copying other inherited entries or asking for an ID in %s', async (locale) => {
  await setLocale(locale);
  const apply = vi.fn<(write: SettingWrite) => void>();
  const original = setting();
  const view = show(<McpEnabledSwitch server={server} setting={original} onApply={apply} />);
  const control = screen.getByRole('switch', { name: i18n.t('mcpSwitch.use', { name: server.name }) });
  expect(control).toBeChecked();
  expect(view.container).not.toHaveTextContent(ID);
  await userEvent.click(control);
  expect(apply).toHaveBeenCalledOnce();
  expect(apply).toHaveBeenCalledWith({ value: { [OTHER]: false, [ID]: false }, expectedRevision: 7 });
  expect(original.own?.value).toEqual({ [OTHER]: false });
  // A controlled setting waits for the parent's confirmed readback.
  expect(control).toBeChecked();
  view.rerender(<McpEnabledSwitch server={server} setting={setting({ value: { [ID]: false } })} onApply={apply} />);
  expect(control).not.toBeChecked();
});

it.each(['en', 'zh-CN'] as const)('uses the native lock flag rather than offering a write in %s', async (locale) => {
  await setLocale(locale);
  const apply = vi.fn();
  show(<McpEnabledSwitch server={server} setting={setting({ writable: false, locked_by: { scope: 'system', node: null } })} onApply={apply} />);
  const control = screen.getByRole('switch');
  expect(control).toBeDisabled();
  expect(screen.getByText(i18n.t('mcpSwitch.locked'))).toBeInTheDocument();
  await userEvent.click(control);
  expect(apply).not.toHaveBeenCalled();
});

it('starts an own entry with the missing-row revision and uses a later confirmed own revision on the next act', async () => {
  const apply = vi.fn();
  const view = show(<McpEnabledSwitch server={server} setting={setting({ own: null, value: {} })} onApply={apply} />);
  await userEvent.click(screen.getByRole('switch'));
  expect(apply).toHaveBeenLastCalledWith({ value: { [ID]: true }, expectedRevision: null });
  view.rerender(<McpEnabledSwitch server={server} setting={setting({ value: { [ID]: true }, own: { revision: 9, locked: false, value: { [ID]: true, [OTHER]: true } } })} onApply={apply} />);
  await userEvent.click(screen.getByRole('switch'));
  expect(apply).toHaveBeenLastCalledWith({ value: { [ID]: false, [OTHER]: true }, expectedRevision: 9 });
});

it('emits one change while the native parent is saving', async () => {
  const apply = vi.fn();
  function Parent() {
    const [pending, setPending] = useState(false);
    return <McpEnabledSwitch server={server} setting={setting()} pending={pending} onApply={(write) => { apply(write); setPending(true); }} />;
  }
  show(<Parent />);
  await userEvent.dblClick(screen.getByRole('switch'));
  expect(apply).toHaveBeenCalledOnce();
  expect(screen.getByRole('switch')).toBeDisabled();
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('mcpSwitch.saving'));
});

it('changes a writable own value without unlocking it or requesting a new lock', async () => {
  const apply = vi.fn();
  show(<McpEnabledSwitch server={server} setting={setting({
    own: { value: { [OTHER]: false }, revision: 7, locked: true },
    source: { scope: 'topic', node: TOPIC },
  })} onApply={apply} />);
  await userEvent.click(screen.getByRole('switch'));
  expect(apply).toHaveBeenCalledWith({ value: { [OTHER]: false, [ID]: false }, expectedRevision: 7 });
  expect(apply.mock.calls[0]?.[0]).not.toHaveProperty('locked');
});

it('cannot switch on a disabled server but can remove an existing use', async () => {
  const apply = vi.fn();
  const disabled = { ...server, enabled: false };
  const view = show(<McpEnabledSwitch server={disabled} setting={setting({ value: {} })} onApply={apply} />);
  expect(screen.getByRole('switch')).toBeDisabled();
  expect(screen.getByText(i18n.t('mcpWords.reasons.disabled'))).toBeInTheDocument();
  view.rerender(<McpEnabledSwitch server={disabled} setting={setting()} onApply={apply} />);
  expect(screen.getByRole('switch')).toBeEnabled();
  await userEvent.click(screen.getByRole('switch'));
  expect(apply).toHaveBeenLastCalledWith({ value: { [OTHER]: false, [ID]: false }, expectedRevision: 7 });
});

it.each([{ case: 'null', value: null }, { case: 'array', value: ['server'] }, { case: 'unknown boolean', value: { [ID]: 'unknown' } }])('has no unchecked fallback or write for $case', ({ value }) => {
  const apply = vi.fn();
  show(<McpEnabledSwitch server={server} setting={setting({ value })} onApply={apply} />);
  expect(screen.queryByRole('switch')).toBeNull();
  expect(screen.getByRole('status')).toHaveTextContent(i18n.t('mcpSwitch.unavailable'));
  expect(apply).not.toHaveBeenCalled();
});

it('never lends another setting row to this control', () => {
  const apply = vi.fn();
  show(<McpEnabledSwitch server={server} setting={setting({ key: 'skills.enabled' })} onApply={apply} />);
  expect(screen.queryByRole('switch')).toBeNull();
  expect(apply).not.toHaveBeenCalled();
});
