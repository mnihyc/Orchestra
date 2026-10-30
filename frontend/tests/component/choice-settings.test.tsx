/** Generic setting choices keep their offered values while naming and describing them in both languages. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import { SettingChoiceContext } from '@/features/settings/choiceCopy';
import { shapeOf } from '@/features/settings/settingsModel';
import { ValueControl } from '@/features/settings/ValueControl';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

describe('catalogued choices in the common setting editor', () => {
  it.each(['en', 'zh-CN'] as const)('offers one named message setting in %s', async (locale) => {
    await setLocale(locale);
    const write = vi.fn();
    const shape = shapeOf({ type: 'array', uniqueItems: true, items: { type: 'string', enum: ['send_message'] } });
    const view = show(<SettingChoiceContext.Provider value="orch_tools.enabled"><ValueControl shape={shape} value={[]} onChange={write} label="Tools" /></SettingChoiceContext.Provider>);
    try {
      const name = i18n.t('choiceCatalog.sharedTool.send_message.name');
      expect(screen.getAllByRole('checkbox')).toHaveLength(1);
      expect(screen.getAllByText(name)).toHaveLength(1);
      await userEvent.click(screen.getByRole('checkbox', { name }));
      expect(write).toHaveBeenCalledExactlyOnceWith(['send_message']);
      expect(screen.queryByText('send_chat_message')).not.toBeInTheDocument();
      expect(screen.queryByText('send_thread_message')).not.toBeInTheDocument();
    } finally { view.unmount(); await setLocale('en'); }
  });

  it('keeps in-page allowed-mode choices compact and offers only the supplied modes', async () => {
    const write = vi.fn();
    const shape = shapeOf({ type: 'array', uniqueItems: true, items: { type: 'string', enum: ['ask', 'on_request'] } });
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        write.mockClear();
        const view = show(<SettingChoiceContext.Provider value="approval.allowed_modes"><ValueControl shape={shape} value={['ask']} onChange={write} label="Modes" /></SettingChoiceContext.Provider>);
        const ask = screen.getByRole('checkbox', { name: i18n.t('composer.mode.labels.ask') });
        expect(ask).toBeChecked();
        expect(ask).toHaveAccessibleDescription(i18n.t('choiceCatalog.approvalMode.ask.description'));
        expect(screen.getByText(i18n.t('choiceCatalog.approvalMode.ask.description'))).toBeVisible();
        const next = screen.getByRole('checkbox', { name: i18n.t('composer.mode.labels.on_request') });
        expect(next).toHaveAccessibleDescription(i18n.t('choiceCatalog.approvalMode.on_request.description'));
        expect(screen.queryByRole('checkbox', { name: i18n.t('composer.mode.labels.auto') })).not.toBeInTheDocument();
        await userEvent.click(next);
        expect(write).toHaveBeenCalledWith(['ask', 'on_request']);
        view.unmount();
      }
    } finally { await setLocale('en'); }
  });

  it('keeps choice context through a nested policy object', () => {
    const shape = shapeOf({ type: 'object', properties: {
      decision: { type: 'string', enum: ['allow', 'ask', 'deny'] },
      approver_scope: { type: 'string', enum: ['environment', 'host', 'org', 'system'] },
    }, required: ['decision', 'approver_scope'], additionalProperties: false });
    show(<SettingChoiceContext.Provider value="capability.policy"><ValueControl shape={shape} value={{ decision: 'allow', approver_scope: 'environment' }} onChange={() => undefined} label="Policy" /></SettingChoiceContext.Provider>);
    const values = screen.getAllByRole<HTMLInputElement>('combobox').map((control) => control.value);
    expect(values).toEqual(expect.arrayContaining(['Allow', 'Environment']));
  });
});
