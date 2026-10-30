/** Resource-page controls look actionable when enabled and explain a genuine refusal by keyboard. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { CommandBox } from '@/features/resources/Facts';
import { HeadingActs } from '@/features/resources/acts';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => {
  vi.restoreAllMocks();
  document.documentElement.removeAttribute('data-theme');
  await setLocale('en');
});

for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  describe(`resource actions in ${theme}, ${locale}`, () => {
    async function prepare() {
      document.documentElement.dataset.theme=theme;
      await setLocale(locale);
      return {user:userEvent.setup(),t:i18n.getFixedT(locale)};
    }

    it('keeps the heading menu enabled and opens its actions with ordinary control contrast', async () => {
      const {user,t}=await prepare();
      const choose=vi.fn();
      show(<HeadingActs name="world-box" primary={null} edit={null}
        menu={[{id:'again',label:t('resources.act.enrollAgain'),onSelect:choose}]} />);
      const menu=screen.getByRole('button',{name:t('resources.act.menuOf',{name:'world-box'})});
      expect(menu).toBeEnabled();
      expect(menu).not.toHaveAttribute('aria-disabled','true');
      expect(menu).toHaveClass('text-text','border-control-border');
      await user.tab();
      expect(menu).toHaveFocus();
      await user.keyboard('{Enter}');
      await user.click(await screen.findByRole('menuitem',{name:t('resources.act.enrollAgain')}));
      expect(choose).toHaveBeenCalledOnce();
    });

    it('explains a blocked primary action on focus and performs no action', async () => {
      const {user,t}=await prepare();
      const refresh=vi.fn();
      const reason=t('machines.probe.offline');
      show(<HeadingActs name="world-box" edit={null} menu={[]}
        primary={{id:'refresh',label:t('resources.act.refresh'),disabled:reason,run:refresh}} />);
      const button=screen.getByRole('button',{name:t('resources.act.refresh')});
      expect(button).toHaveAttribute('aria-disabled','true');
      await user.tab();
      expect(button).toHaveFocus();
      expect(await screen.findByRole('tooltip')).toHaveTextContent(reason);
      expect(button).toHaveAccessibleDescription(reason);
      await user.keyboard('{Enter}');
      expect(refresh).not.toHaveBeenCalled();
    });

    it('copies the machine command by keyboard and announces success', async () => {
      const {user,t}=await prepare();
      const write=vi.spyOn(navigator.clipboard,'writeText').mockResolvedValue(undefined);
      const command="orch-connector policy delegate 'environment.*'";
      show(<CommandBox command={command} label={t('machines.policy.copy')} />);
      const button=screen.getByRole('button',{name:t('machines.policy.copy')});
      expect(button).toBeEnabled();
      expect(button).not.toHaveAttribute('aria-disabled','true');
      expect(button).toHaveClass('text-text');
      await user.tab();
      expect(button).toHaveFocus();
      await user.keyboard('{Enter}');
      expect(write).toHaveBeenCalledWith(command);
      expect(await within(button).findByRole('status')).toHaveTextContent(t('common.copied'));
    });
  });
}
