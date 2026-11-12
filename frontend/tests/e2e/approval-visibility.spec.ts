/** Native rule/read and unavailable-choice presentation through the actual inspector and settings entrances. */
import { expect, type Page, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef, ThreadApprovalRule } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn, supplyDefaults } from './support';
import { showWorkbenchView } from './workbench-support';

async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
}

function rule(id: string, reason: string): ThreadApprovalRule {
  return { id, reason, active: false, revision: 2, at: '2026-11-09T06:00:00Z',
    matcher: { type: 'command_exact', command: 'git status', cwd: '/workspace' },
    by: { id: '00000000-0000-4000-8001-000000000001', username: 'ada', display_name: 'Ada Lovelace', kind: 'human', avatar: { text: 'AL', color: 2 } } };
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1280]) {
  test(`remembered rules stay visible on checking, failed read and native inactive causes ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    let phase: 'checking' | 'failed' | 'ready' = 'checking';
    let release: (() => void) | undefined;
    const held = new Promise<void>((resolve) => { release = resolve; });
    const methods: string[] = [];
    await page.route(`**/api/v1/threads/${ACME.mainThread}/approval-rules*`, async (route) => {
      methods.push(route.request().method());
      expect(route.request().method()).toBe('GET');
      if (phase === 'checking') await held;
      if (phase !== 'ready') {
        await route.fulfill({ status: 503, json: { type: 'orch:error:source.offline', code: 'source.offline', title: 'Unavailable',
          detail: 'Unavailable', status: 503, request_id: 'rule-read' } });
        return;
      }
      await route.fulfill({ json: { items: [rule('00000000-0000-4000-8002-000000000001', 'revoked'),
        rule('00000000-0000-4000-8002-000000000002', 'binding_changed'), rule('00000000-0000-4000-8002-000000000003', 'private-native-reference')], next_cursor: null } });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    await showWorkbenchView(page, 'Parameters');
    const section = page.getByTestId('parameters-panel').getByRole('region', { name: words.approvals.rules.title, exact: true });
    await expect(section.getByText(words.approvals.rules.checking, { exact: true })).toBeVisible();
    await expect(section.getByText(words.approvals.rules.none, { exact: true })).toHaveCount(0);
    await info.attach('rules-checking', { body: await section.screenshot(), contentType: 'image/png' });
    phase = 'failed';
    if (release === undefined) throw new Error('The rule read was not held');
    release();
    await expect(section.getByText(words.approvals.rules.unavailable, { exact: true })).toBeVisible();
    await expect(section.getByText(words.approvals.rules.none, { exact: true })).toHaveCount(0);
    phase = 'ready';
    await section.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await expect(section.getByText(words.approvals.rules.reasonRevoked, { exact: true })).toBeVisible();
    await expect(section.getByText(words.approvals.rules.reasonBindingChanged, { exact: true })).toBeVisible();
    await expect(section.getByText(words.approvals.rules.reasonUnavailable, { exact: true })).toBeVisible();
    await expect(section).not.toContainText('private-native-reference');
    expect(methods.length).toBeGreaterThanOrEqual(2);
    expect(methods.every((method) => method === 'GET')).toBe(true);
    const bounds = await section.boundingBox();
    if (bounds === null) throw new Error('The entitled rules inspector is missing');
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
    await info.attach('rules-inactive-causes', { body: await section.screenshot(), contentType: 'image/png' });
    // EN: Thread → Parameters → Checking rules → read failure → Try again → Taken back / Setup changed.
    // 中文：会话 → 参数 → 正在检查规则 → 读取失败 → 重试 → 已收回／运行配置已更改。
  });

  test(`settings retain unsupported Auto with the native program reason and no write ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const words = locale === 'en' ? english : chinese;
    const schema = { type: 'string', enum: ['ask', 'on_request', 'bypass'],
      'x-unavailable-choices': [{ value: 'auto', reason: 'program_auto_unavailable' }] };
    const definition: SettingDef = { key: 'approval.mode', title: 'Approval mode', description: 'How native permission requests are answered.',
      scopes: ['org'], group: 'confinement', class: 'parameter', merge: 'replace', schema, default: 'on_request', example: null, text_key: null, keywords: [] };
    const allowed: SettingDef = { ...definition, key: 'approval.allowed_modes', title: 'Allowed approval modes', class: 'policy', merge: 'intersect',
      schema: { type: 'array', uniqueItems: true, items: schema }, default: ['ask', 'on_request', 'auto', 'bypass'] };
    // The allowed modes were narrowed here before the program lost Auto: their own part holds it, which a narrowing editor shows.
    const research = { id: ACME.research, kind: 'org' as const, name: 'Research', redacted: false };
    const settings: ResolvedSetting[] = [definition, allowed].map((def) => def.key === allowed.key
      ? { key: def.key, value: def.default, own: { value: def.default, locked: false, revision: 1 }, writable: true, source: { scope: 'org', node: research }, locked_by: null, parts: [] }
      : { key: def.key, value: def.default, own: null, writable: true, source: { scope: 'default', node: null }, locked_by: null, parts: [] });
    const writes: string[] = [];
    // These are native presentation facts. The Rust producer cases decide which programs may offer Auto.
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [definition, allowed] }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: settings }));
    await supplyDefaults(page, ACME.research, { registry: () => [definition, allowed], settings: () => settings });
    await page.route((url) => url.pathname.startsWith(`/api/v1/nodes/${ACME.research}/settings/`), async (route) => {
      if (route.request().method() === 'GET') { await route.fallback(); return; }
      writes.push(route.request().method());
      throw new Error('Opening an unsupported choice must never write a setting');
    });
    // The canonical setting link keeps this focused editor independent of a shared full-defaults opening.
    await page.goto('/org/acme.research/settings/defaults?setting=approval.mode');
    const row = page.locator('[data-setting="approval.mode"]');
    await row.getByRole('combobox').click();
    const automatic = page.getByRole('option', { name: words.choiceCatalog.approvalMode.auto.name, exact: true });
    await expect(automatic).toBeVisible();
    await expect(automatic).toHaveAttribute('aria-disabled', 'true');
    await automatic.hover();
    await expect(page.getByText(words.settings.editor.autoUnavailable, { exact: true })).toBeVisible();
    await info.attach('settings-auto-unavailable', { body: await page.screenshot(), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await page.goto('/org/acme.research/settings/defaults?setting=approval.allowed_modes');
    const modes = page.locator('[data-setting="approval.allowed_modes"]');
    await modes.getByRole('button', { name: words.settings.compact.editValue.replace('{{name}}', words.settingRegistry.approval.allowed_modes.title), exact: true }).click();
    const retained = modes.getByRole('checkbox', { name: words.choiceCatalog.approvalMode.auto.name, exact: true });
    await expect(retained).toBeChecked();
    await expect(retained).not.toBeDisabled();
    await expect(modes.getByText(words.settings.editor.autoUnavailable, { exact: true })).toBeVisible();
    expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('settings-retained-auto', { body: await modes.screenshot(), contentType: 'image/png' });
    // EN: Org → Settings → Approval mode → Auto (disabled, program reason); Allowed approval modes → edit → retained Auto unchanged.
    // 中文：组织 → 设置 → 审批模式 → 自动（禁用并显示程序原因）；允许的审批模式 → 编辑 → 保留的自动值不变。
  });
}
