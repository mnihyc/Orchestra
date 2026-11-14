/** Availability and automatic choices stay visible without changing the native operation. */
import { expect, test } from '@playwright/test';
import type { FileStat, ThreadOpening } from '../../src/generated/api-types';
import { language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Topic → Thread setup → Options → directory offline → Retry → Exists → refused → missing.
  // 主题 → 会话设置 → 选项 → 目录离线未检查 → 重试 → 已存在 → 访问被拒绝 → 路径不存在。
  test(`working directory check stays explicit ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const t = await language(page, request, locale, theme);
    const reads: string[] = []; let workReady = false;
    await page.route('**/environments/*/files/stat?**', async (route) => {
      const path = new URL(route.request().url()).searchParams.get('path');
      if (!['work', 'private', 'missing'].includes(path ?? '')) { await route.continue(); return; }
      reads.push(path ?? '');
      if (path === 'work' && workReady) {
        const stat: FileStat = { cached: false, hash: null, is_text: false, link_target: null, media_type: 'inode/directory', mode: 493,
          mtime: '2026-11-08T00:00:00Z', name: 'work', path: 'work', size: 0, type: 'dir' };
        await route.fulfill({ json: stat }); return;
      }
      const status = path === 'private' ? 403 : path === 'missing' ? 404 : 503;
      await route.fulfill({ status, json: { type: 'orch:error', code: status === 403 ? 'authz.denied' : status === 404 ? 'not_found' : 'source.offline',
        status, title: '', detail: 'Hidden source and grant canary', request_id: 'directory-availability' } });
    });
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.locator('[data-quick-start]').getByRole('button', { name: t('start.quick.change'), exact: true }).click();
    const form = page.getByRole('dialog', { name: t('start.title', { parent: 'Fix login' }), exact: true });
    await form.getByRole('button', { name: t('start.options.title'), exact: true }).click();
    const existing = form.getByRole('radio', { name: t('start.where.useExisting'), exact: true });
    if (await existing.isVisible()) await existing.click();
    const directory = form.getByRole('textbox', { name: t('start.where.directory') });
    await directory.fill('work');
    const sign = form.locator('[data-directory-sign="unchecked"]');
    await expect(sign).toContainText(t('start.where.directoryNotChecked')); await expect(sign).toContainText(t('start.where.directoryOffline'));
    await expect(form.locator('[data-directory-sign="new"]')).toHaveCount(0); await expect(directory).toHaveValue('work');
    await sign.scrollIntoViewIfNeeded();
    const bounds = await sign.boundingBox(), frame = await form.boundingBox();
    if (bounds === null || frame === null) throw new Error('The directory check is laid out inside its form');
    expect(bounds.x).toBeGreaterThanOrEqual(frame.x); expect(bounds.x + bounds.width).toBeLessThanOrEqual(frame.x + frame.width + 1);
    await info.attach('directory-unchecked', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    workReady = true; await sign.getByRole('button', { name: t('common.tryAgain'), exact: true }).click();
    await expect(form.locator('[data-directory-sign="exists"]')).toHaveText(t('start.where.directoryExists'));
    await directory.fill('private'); await expect(sign).toContainText(t('start.where.directoryNoAccess'));
    await expect(form).not.toContainText('Hidden source and grant canary');
    await expect(form.locator('[data-directory-sign="new"]')).toHaveCount(0);
    await directory.fill('missing'); await expect(form.locator('[data-directory-sign="new"]')).toHaveText(t('start.where.directoryNew'));
    expect(reads).toEqual(['work', 'work', 'private', 'missing']);
    await info.attach('directory-check-reads', { body: JSON.stringify({ reads }), contentType: 'application/json' });
    await form.getByRole('button', { name: t('common.cancel'), exact: true }).click();
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Thread → model → forbidden Standard/None reasons → unknown context → Parameters → known context returns.
  // 会话 → 模型 → 标准/无的禁用原因 → 上下文范围未知 → 参数 → 恢复已知范围。
  test(`model restrictions and missing context stay explicit ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const t = await language(page, request, locale, theme); let knownContext = false; let providerName = '';
    await page.route(`**/threads/${ACME.mainThread}/opening?**`, async (route) => {
      const response = await route.fetch(); const opening = await response.json() as ThreadOpening;
      const parameters = opening.parameters;
      if (parameters === null) throw new Error('The readable fixture includes thread parameters');
      if (parameters.provider === undefined || parameters.provider === null) throw new Error('The fixture has a readable restriction source');
      providerName = parameters.provider.name;
      const model = parameters.model.options.find((candidate) => candidate.model_name === parameters.model.value);
      if (model === undefined) throw new Error('The current model is a readable option');
      model.parameters = { ...model.parameters, service_tiers: [{ id: 'fast', label: 'Fast' }], default_service_tier: 'fast',
        access_programs: ['Daybreak Blue'], default_access_program: 'Daybreak Blue', context_max: knownContext ? 100_000 : null, default_context: knownContext ? 100_000 : null };
      const source = { scope: 'default' as const, node: null };
      parameters.parameters = [
        { key: 'service_tier', def: { key: 'service_tier', title: 'Speed', options: { from_model: 'service_tiers' }, resolved_options: [{ value: 'fast', label: 'Fast' }], allows_none: false, constraint_source: 'provider', provider_constraint: { enum: ['fast'] } }, value: 'fast', source, locked_by: null, writable: true, apply: 'next_turn' },
        { key: 'cyber_access_program', def: { key: 'cyber_access_program', title: 'Access', options: { from_model: 'access_programs' }, resolved_options: [{ value: 'Daybreak Blue', label: 'Daybreak Blue' }], allows_none: false, constraint_source: 'provider', provider_constraint: { enum: ['Daybreak Blue'] } }, value: 'Daybreak Blue', source, locked_by: null, writable: true, apply: 'next_turn' },
        { key: 'context', def: { key: 'context', title: 'Context', type: 'integer', range: { min: 32_000, max: { from_model: 'context_max' } } }, value: null, source, locked_by: null, writable: true, apply: 'new_session' },
      ];
      await route.fulfill({ response, json: opening });
    });
    await page.goto(`/thread/${ACME.mainThread}`);
    // The composer row has no control for a speed, a program or a context where there is nothing to choose; the Parameters view says what the provider allows.
    await page.getByRole('button', { name: /^(Add a view|添加视图)$/, exact: true }).first().click();
    await page.getByRole('dialog', { name: /^(Views|视图)$/, exact: true }).locator('[data-value="commands:view.open.parameters"]').click();
    const inspector = page.getByTestId('parameters-panel');
    const standard = inspector.getByRole('radio', { name: t('composer.model.speed.standard'), exact: true });
    await expect(standard).toBeDisabled(); await standard.hover();
    await expect(page.getByRole('tooltip')).toContainText(t('composer.model.notAmongAllowedValues'));
    await expect(page.getByRole('tooltip')).toContainText(providerName);
    await page.mouse.move(0, 0);
    await inspector.getByRole('combobox', { name: t('composer.model.access.title'), exact: true }).click();
    const none = page.getByRole('option', { name: t('composer.model.access.none'), exact: true });
    await expect(none).toHaveAttribute('aria-disabled', 'true');
    await expect(none).toContainText(t('composer.model.notAmongAllowedValues')); await expect(none).toContainText(providerName);
    await info.attach('access-refusal', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await expect(inspector.locator('[data-context-availability="limits"]')).toContainText(t('composer.model.context.unreportedLimits'));
    await expect(inspector.getByRole('textbox', { name: t('composer.model.context.another') })).toHaveCount(0);
    await info.attach('parameters-context-unknown', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    if (width === 400) await page.locator('[data-workbench-bar="right"]:visible').getByRole('button', { name: t('common.close'), exact: true }).click();
    knownContext = true; await page.reload();
    await page.getByRole('button', { name: /^(Add a view|添加视图)$/, exact: true }).first().click();
    await page.getByRole('dialog', { name: /^(Views|视图)$/, exact: true }).locator('[data-value="commands:view.open.parameters"]').click();
    const known = page.getByTestId('parameters-panel');
    await expect(known.locator('[data-context-availability]')).toHaveCount(0);
    await expect(known.getByRole('radio', { name: '100K', exact: true })).toBeChecked();
  });
}
