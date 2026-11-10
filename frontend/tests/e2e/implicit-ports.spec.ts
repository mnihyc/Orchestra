/** Bilingual click paths keep native Ports refusals visible without widening their audience. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, deny, scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';
import { RUNS_ON } from '../doubles/fixtures';

for (const locale of ['en', 'zh-CN'] as const) {
  test(`an already open Ports panel explains loss of its served permission ${locale}`, async ({ page }) => {
    await scenario(); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${ACME.mainThread}`);
    await showWorkbenchView(page, words.ports.title);
    await expect(page.getByTestId('ports-panel')).toBeVisible();
    await deny('ada', ['ports.view']);
    await page.reload();
    await expect(page.getByText(words.ports.view.denied, { exact: true })).toBeVisible();
    await expect(page.getByTestId('ports-panel')).toHaveCount(0);
    // EN: Thread → Ports → lose Ports View → Reload → Ports View required; no retained port inventory.
    // 中文：会话 → 端口 → 撤销端口查看权限 → 刷新 → 需要端口查看权限；不保留端口清单。
  });
  test(`port capability actions remain visible with native Manage reasons and their existing confirmation ${locale}`, async ({ page }) => {
    await scenario(); await deny('ada', ['ports.manage']); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/environments/${RUNS_ON.sandbox}?tab=ports`);
    const row = page.getByTestId('port-grant-place');
    const grant = row.getByRole('button', { name: words.ports.grants.grant, exact: true });
    const revoke = row.getByRole('button', { name: words.ports.grants.revoke, exact: true });
    await expect(grant).toHaveAttribute('aria-disabled', 'true');
    await expect(grant).toHaveAttribute('aria-description', words.ports.grants.reasons.manage);
    await expect(revoke).toHaveAttribute('aria-disabled', 'true');
    await grant.focus(); await page.keyboard.press('Enter');
    await expect(page.getByRole('dialog')).toHaveCount(0);
    await deny('ada', []); await page.reload();
    await expect(grant).not.toHaveAttribute('aria-disabled', 'true');
    await grant.click();
    const confirmation = page.getByRole('dialog');
    await expect(confirmation).toBeVisible();
    await confirmation.getByRole('button', { name: words.ports.grants.grant, exact: true }).click();
    await expect(grant).toHaveAttribute('aria-disabled', 'true');
    await expect(grant).toHaveAttribute('aria-description', words.ports.grants.reasons.granted);
    await expect(revoke).not.toHaveAttribute('aria-disabled', 'true');
    // EN: Environment → Ports → Grant/Take back disabled with Ports Manage required → restore → Grant → existing confirmation.
    // 中文：环境 → 端口 → 授予/收回均显示需要端口管理权限 → 恢复权限 → 授予 → 原有确认。
  });
  test(`the thread inspector names inactive requested port tools without losing Close ${locale}`, async ({ page }) => {
    await scenario(); await signIn(page);
    const status = { requested: ['list_ports', 'open_port', 'close_port'], effective: ['close_port'],
      inactive: [{ tool: 'list_ports', reason: 'no_machine' }, { tool: 'open_port', reason: 'no_machine' }] };
    await page.route(`**/api/v1/threads/${ACME.mainThread}/parameters`, async (route) => {
      const response = await route.fetch(); const body: unknown = await response.json();
      expect(response.ok()).toBe(true);
      if (typeof body !== 'object' || body === null) throw new Error('the fixture needs native parameters');
      await route.fulfill({ response, json: { ...body, port_tools: status } });
    });
    await page.route(`**/api/v1/threads/${ACME.mainThread}/opening**`, async (route) => {
      const response = await route.fetch(); const body: unknown = await response.json();
      if (typeof body !== 'object' || body === null || !('parameters' in body)) throw new Error('the fixture needs its native opening');
      const parameters = typeof body.parameters === 'object' && body.parameters !== null ? { ...body.parameters, port_tools: status } : body.parameters;
      await route.fulfill({ response, json: { ...body, parameters } });
    });
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${ACME.mainThread}`);
    await showWorkbenchView(page, words.views.parameters.title);
    const inspector = page.getByRole('region', { name: words.ports.tools.title, exact: true });
    await expect(inspector).toBeVisible();
    await expect(inspector.locator('[data-port-tool="list_ports"]')).toContainText(words.ports.tools.reasons.noMachine);
    await expect(inspector.locator('[data-port-tool="open_port"]')).toContainText(words.ports.tools.inactive);
    await expect(inspector.locator('[data-port-tool="close_port"]')).toContainText(words.ports.tools.available);
    // EN: Thread → Parameters → Port tools → requested reads/open inactive for No current machine; Close remains Available.
    // 中文：会话 → 参数 → 端口工具 → 读取/开放因尚无当前机器而未生效；收回开放入口仍可用。
  });
}
