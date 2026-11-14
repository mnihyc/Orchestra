/** Creation presets keep exact Share areas and use ordinary setting edits, not raw JSON. */
import { useState } from 'react';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { SettingValue } from '@/features/settings/SettingValue';
import { SettingSummary } from '@/features/settings/SettingSummary';
import { AgentAccessControl, agentAccessValue } from '@/features/access/AgentAccessChoice';
import { shapeOf } from '@/features/settings/settingsModel';
import type { AgentAccessChoice, ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n, setLocale } from '@/i18n';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const definition = SETTINGS_REGISTRY.find((row) => row.key === 'threads.agent_access');
if (definition === undefined) throw new Error('The creation preset is registered');
const preset: AgentAccessChoice = { where: 'topic', parts: { documents: 'edit' } };
const edited = vi.fn();
function Editor({ disabled = false }: { disabled?: boolean }) {
  const [value, setValue] = useState<unknown>(preset);
  const setting: ResolvedSetting = { key: 'threads.agent_access', value: preset, own: null, writable: !disabled,
    locked_by: null, source: { scope: 'default', node: null }, parts: [] };
  if (definition === undefined) throw new Error('The creation preset is registered');
  return <><SettingValue def={definition} setting={setting} target={{ kind: 'node', id: 'topic' }}
    shape={shapeOf(definition.schema)} label={i18n.t('agentAccess.title')} value={value} disabled={disabled}
    onChange={(next) => { edited(next); setValue(next); }} />
    <output aria-label="Confirmed preset">{JSON.stringify(value)}</output></>;
}

it('uses the currently edited reach and keeps Documents-only giving available without a whole Edit preset', async () => {
  const changed = vi.fn();
  function Controlled() {
    const [value, setValue] = useState(preset);
    const [ready, setReady] = useState(false);
    return <><AgentAccessControl value={value} onChange={(next) => { changed(next); setValue(next); }}
      available={(where) => where === 'topic' ? { documents: 'edit' } : where === 'project' && ready ? { documents: 'read' } : undefined} />
      <button onClick={() => setReady(true)}>Current project bounds arrived</button></>;
  }
  show(<Controlled />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('agentAccess.change') }));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.agents.area.documents') }));
  expect(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.edit.name') })).not.toHaveAttribute('aria-disabled', 'true');
  await userEvent.keyboard('{Escape}');
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('agentAccess.scope') }));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('agentAccess.where.project') }));
  const confirm = screen.getByRole('button', { name: i18n.t('agentAccess.confirm') });
  expect(confirm).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(screen.getByRole('button', { name: 'Current project bounds arrived' }));
  expect(confirm).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(screen.getByRole('combobox', { name: i18n.t('access.agents.area.documents') }));
  await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.agentStep.view.name') }));
  await userEvent.click(confirm);
  expect(changed).toHaveBeenCalledExactlyOnceWith({ where: 'project', parts: { documents: 'read' } });
  expect(screen.getByRole('button', { name: i18n.t('agentAccess.change') })).toHaveFocus();
});

it.each([
  { where: 'nothing', parts: { documents: 'edit' } },
  { where: 'thread', parts: { timeline: 'write' } },
  { where: 'topic', parts: { documents: 'own' } },
  { where: 'project', parts: { documents: 'edit', persona: 'human' } },
])('keeps an incoherent or unknown preset out of the friendly grant editor: %j', (value) => {
  expect(agentAccessValue(value)).toBeNull();
});
afterEach(async () => { edited.mockClear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`edits the Documents-only preset without adding chats or other threads (${locale})`, async () => {
    await setLocale(locale);
    show(<Editor />);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('agentAccess.change') }));
    const reach = screen.getByRole('combobox', { name: i18n.t('agentAccess.scope') });
    expect(reach).toHaveTextContent(i18n.t('agentAccess.where.topic'));
    expect(screen.getByRole('combobox', { name: i18n.t('access.agents.dialog.what') })).toHaveTextContent(i18n.t('choiceCatalog.generalAccess.custom.name'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('access.agents.dialog.byArea') }));
    const areas = screen.getByRole('group', { name: i18n.t('access.agents.dialog.byAreaLabel') });
    const rows = within(areas).getAllByRole('listitem');
    expect(rows).toHaveLength(3);
    for (const [index, area] of ['timeline', 'chat', 'documents'].entries()) {
      const row = rows[index];
      if (row === undefined) throw new Error('Each preset area keeps its own readable control.');
      expect(within(row).getByRole('combobox', { name: i18n.t(`access.agents.area.${area}`) })).toBeVisible();
    }
    expect(within(areas).getByRole('combobox', { name: i18n.t('access.agents.area.timeline') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.none.name'));
    expect(within(areas).getByRole('combobox', { name: i18n.t('access.agents.area.chat') })).toHaveTextContent(i18n.t('choiceCatalog.agentStep.none.name'));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
    expect(edited).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Confirmed preset')).toHaveTextContent(JSON.stringify(preset));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('agentAccess.change') }));
    await userEvent.click(screen.getByRole('combobox', { name: i18n.t('agentAccess.scope') }));
    await userEvent.click(screen.getByRole('option', { name: i18n.t('agentAccess.where.nothing') }));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('agentAccess.confirm') }));
    expect(edited).toHaveBeenLastCalledWith({ where: 'nothing', parts: {} });
  });

  it(`summarizes exact areas and keeps a locked preset unchanged (${locale})`, async () => {
    await setLocale(locale);
    const summary = show(<SettingSummary def={definition} value={preset} />);
    expect(summary.container).toHaveTextContent(i18n.t('agentAccess.where.topic'));
    expect(summary.container).toHaveTextContent(i18n.t('access.agents.area.documents'));
    expect(summary.container).not.toHaveTextContent('"where"');
    summary.unmount();
    show(<Editor disabled />);
    expect(screen.getByRole('button', { name: i18n.t('agentAccess.change') })).toBeDisabled();
    expect(edited).not.toHaveBeenCalled();
  });
}
