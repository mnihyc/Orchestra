/** Ask click paths use product controls in both languages and the published server contract. */
import { expect, test } from '@playwright/test';
import { id } from '../doubles/fixtures';
import { DOCS } from './documents-support';
import { ACME, PEOPLE, scenario } from './support';
import { fixture, language } from './ask-support';
import { ASK_ID, askConversation, askMessage, askSnapshot } from '../unit/ask-fixtures';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`Ask: page → send → stop → history → citation → keep (${locale}, ${width}, ${theme})`, async ({ page, request }, info) => {
    const unregisteredAsk: string[] = [];
    page.on('console', (message) => {
      const text = message.text();
      if (message.type() === 'error' && text.startsWith('An act has no entry in the registry of acts') && text.includes('POST /ask/conversations')) unregisteredAsk.push(text);
    });
    await scenario(); await page.setViewportSize({ width, height: 900 });
    const server = await fixture(page), t = await language(page, request, locale, theme);
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    if (width === 1440) await expect(page.getByRole('button', { name: t('ask.open'), exact: true })).toHaveAttribute('aria-pressed', 'true');
    const panel = page.locator('[data-view-window="ask"]');
    await expect(panel.getByText(t('ask.empty'), { exact: true })).toBeVisible();
    const box = panel.getByRole('textbox', { name: t('ask.composer') });
    await box.fill('What changed?'); await box.press('Shift+Enter'); await box.pressSequentially('Give sources');
    await expect(box).toHaveValue('What changed?\nGive sources');
    await box.pressSequentially(' @');
    const resources = page.getByRole('dialog', { name: t('ask.resources'), exact: true });
    await resources.getByRole('combobox', { name: t('ask.findResource'), exact: true }).fill('Rollout');
    await page.getByRole('option', { name: /Rollout plan/ }).click();
    await expect(box).toHaveValue(new RegExp(DOCS.plan));
    await panel.locator('input[type="file"]').setInputFiles({ name: 'review.txt', mimeType: 'text/plain', buffer: Buffer.from('Local notes') });
    await expect(panel.getByText('review.txt', { exact: true }).first()).toBeVisible();
    await expect(panel.getByRole('button', { name: t('composer.send'), exact: true })).toBeEnabled();
    await box.press('Enter');
    await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toBeVisible();
    expect(server.creates).toHaveLength(1); expect(server.creates[0]).toMatchObject({ root_org_id: ACME.org, model: null, retention_days: null });
    expect(server.sends[0]?.context).toContain(`/topic/${ACME.fixLogin}`);
    expect(server.sends[0]?.model).toBeNull();
    expect(server.sends[0]?.content.some((part) => part.type === 'file' && part.name === 'review.txt')).toBe(true);
    await server.foreignNotice();
    await server.delta('你😀'); await expect(panel.locator('[data-role="assistant"]')).toContainText('你😀');
    await expect(panel.locator('[data-ask-message][data-role="user"]').getByText('review.txt', { exact: true })).toBeVisible();
    await expect(panel).not.toContainText('Another organization note');
    await panel.getByRole('button', { name: t('composer.stop'), exact: true }).click();
    await expect(panel.getByText(t('ask.stopped'), { exact: true })).toBeVisible(); expect(server.stops).toBe(1);
    await server.finish();
    await expect(panel.getByRole('list', { name: t('ask.sources') })).toBeVisible();
    await expect(panel.getByText(t('ask.adminNotice'), { exact: true })).toBeVisible();
    await panel.getByRole('button', { name: t('ask.history'), exact: true }).click();
    const history = page.getByRole('dialog', { name: t('ask.history'), exact: true });
    await history.getByRole('button', { name: /Release questions/ }).click(); await expect(history).toHaveCount(0);
    await panel.getByRole('button', { name: t('viewWindow.popOut'), exact: true }).click();
    await expect(panel).toHaveAttribute('data-floating', 'true');
    await box.fill('Keep this draft');
    const resize = panel.getByRole('button', { name: t('viewWindow.resize', { name: 'Release questions' }) });
    await resize.focus(); await resize.press('ArrowRight'); await resize.press('ArrowDown');
    const handle = await resize.boundingBox();
    if (handle === null) throw new Error('The window has a resize handle');
    await page.mouse.move(handle.x + handle.width / 2, handle.y + handle.height / 2); await page.mouse.down();
    await page.mouse.move(handle.x + handle.width / 2, 8); await page.mouse.up();
    await expect(box).toBeInViewport(); await expect(panel.getByRole('button', { name: t('composer.send'), exact: true })).toBeInViewport();
    await panel.getByRole('button', { name: t('viewWindow.maximize'), exact: true }).click();
    await panel.getByRole('button', { name: t('viewWindow.restore'), exact: true }).click();
    await panel.getByRole('button', { name: t('viewWindow.dock'), exact: true }).click(); await expect(box).toHaveValue('Keep this draft');
    const bounds = await panel.boundingBox(); expect(bounds).not.toBeNull();
    if (bounds !== null) expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
    await page.screenshot({ path: info.outputPath(`ask-${locale}-${width}-${theme}.png`), animations: 'disabled' });
    await panel.getByRole('button', { name: t('ask.keep'), exact: true }).click();
    const keep = page.getByRole('dialog', { name: t('ask.keep'), exact: true });
    await expect(keep.getByText(t('ask.privateThread'), { exact: true })).toBeVisible();
    await keep.getByRole('button', { name: t('ask.keep'), exact: true }).click();
    await expect.poll(() => new URL(page.url()).pathname).toBe(`/thread/${ACME.depsThread}`);
    expect(server.keeps).toEqual([{ topic_id: null, project_id: null, title: null }]);
    expect(unregisteredAsk).toEqual([]);
  });
}

for (const locale of ['en', 'zh-CN'] as const) test(`Ask switches conversations when the page changes root (${locale})`, async ({ page, request }) => {
  await scenario(); const server = await fixture(page), t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`); await page.keyboard.press('Shift+F1');
  const panel = page.locator('[data-view-window="ask"]');
  await panel.getByRole('textbox', { name: t('ask.composer') }).fill('Acme question'); await panel.getByRole('textbox', { name: t('ask.composer') }).press('Enter');
  await expect(panel.getByText(t('ask.running'), { exact: true })).toBeVisible();
  await page.goto('/org/ada');
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  await expect(panel.getByText(t('ask.empty'), { exact: true })).toBeVisible(); await expect(panel).not.toContainText('Acme question');
  const box = panel.getByRole('textbox', { name: t('ask.composer') }); await box.fill('Personal question'); await box.press('Enter');
  await expect.poll(() => server.creates.length).toBe(2);
  expect(server.creates[1]?.root_org_id).toBe(id(2, 1)); expect(server.sends[1]?.context).toContain('/org/ada');
  await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  await expect(panel).toContainText('Acme question'); await expect(panel).not.toContainText('Personal question');
});

for (const locale of ['en', 'zh-CN'] as const) test(`Ask model choices do not require thread creation authority (${locale})`, async ({ page, request }) => {
  await scenario(); await fixture(page);
  const t = await language(page, request, locale, 'light', PEOPLE.vera);
  const wrongReads: string[] = [];
  await page.route(`**/api/v1/nodes/${ACME.org}/binding/options*`, async (route) => {
    wrongReads.push(route.request().url());
    await route.fulfill({ status: 400, json: { status: 400, code: 'request.invalid', message: 'A thread starts in a topic, project or folder', request_id: 'fixture-ask-models' } });
  });
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  await page.locator('[data-view-window="ask"]').getByRole('button', { name: t('ask.model'), exact: true }).click();
  const picker = page.getByRole('dialog', { name: t('ask.model'), exact: true });
  await expect(picker.getByRole('menuitemradio', { name: 'Readable API model', exact: true })).toBeEnabled();
  await expect(picker.getByRole('menuitemradio', { name: /Excluded API model/ })).toHaveCount(0);
  await picker.getByRole('menuitemradio', { name: 'Readable API model', exact: true }).click();
  await page.waitForLoadState('networkidle');
  expect(wrongReads, 'Ask is a root-scoped read; normal thread-start choices reject orgs and require unrelated creation rights').toEqual([]);
});

for (const locale of ['en', 'zh-CN'] as const) test(`Ask autosaves and restores a private draft without starting a turn (${locale})`, async ({ page, request }) => {
  await scenario(); const server = await fixture(page), t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  const panel = page.locator('[data-view-window="ask"]'), text = 'An unsent private question';
  await panel.getByRole('textbox', { name: t('ask.composer') }).fill(text);
  await expect.poll(() => server.drafts.get(ASK_ID)?.content).toEqual([{ type: 'text', text }]);
  expect(server.creates).toHaveLength(1); expect(server.sends).toHaveLength(0);
  page.once('dialog', (dialog) => dialog.accept()); await page.reload();
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  await expect(panel.getByRole('textbox', { name: t('ask.composer') })).toHaveValue(text);
  expect(server.sends).toHaveLength(0);
});

for (const locale of ['en', 'zh-CN'] as const) test(`As admin opens a read-only Ask inspection and retires it when mode is off (${locale})`, async ({ page, request }) => {
  await scenario();
  const inspected = askSnapshot({ conversation: askConversation({ title: 'Inspected private question', user_id: id(1, 2), inspecting: true, can: { message: false, stop: false, keep: false, delete: false, export: true } }), messages: [askMessage({ content: [{ type: 'text', text: 'A permitted inspection answer' }] })] });
  const server = await fixture(page, [inspected]), t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('switch', { name: t('adminReach.mode.label', { state: t('adminReach.mode.stateOff') }), exact: true }).click();
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  const panel = page.locator('[data-view-window="ask"]'); await panel.getByRole('button', { name: t('ask.history'), exact: true }).click();
  const history = page.getByRole('dialog', { name: t('ask.history'), exact: true });
  await history.getByRole('checkbox', { name: t('ask.inspectOrg'), exact: true }).check();
  await history.getByRole('button', { name: /Inspected private question/ }).click();
  await expect(panel).toContainText('A permitted inspection answer'); await expect(panel.getByRole('textbox', { name: t('ask.composer') })).toBeDisabled();
  await expect(panel.getByRole('button', { name: t('ask.keep'), exact: true })).toBeDisabled();
  const downloaded = page.waitForEvent('download'); await panel.getByRole('button', { name: t('ask.export'), exact: true }).click();
  expect((await downloaded).suggestedFilename()).toMatch(/\.json$/);
  await page.getByRole('switch', { name: t('adminReach.mode.label', { state: t('adminReach.mode.stateOn') }), exact: true }).click();
  await expect(panel).not.toContainText('A permitted inspection answer'); await expect(panel).toContainText(t('ask.inspectEnable')); expect(server.sends).toHaveLength(0);
});
