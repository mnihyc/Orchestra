/** Production UI against explicit continuation DTOs; native preparation has separate integration proof. */
import { expect, test } from '@playwright/test';
import { begin, MAIN, openThread } from './inspector-support';
import { A, B, INPUT_B, ITEM_A, versions } from './prompt-version-support';

for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`edited messages browse with compact version arrows at ${width} in ${theme}`, async ({ page }, info) => {
    await begin(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const { calls } = await versions(page); const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await openThread(page);
    const strip = page.getByRole('group', { name: 'Message versions', exact: true });
    await expect(strip).toContainText('2/2');
    await expect(page.locator('[data-screen="thread"]').getByText('Revised release plan', { exact: true })).toBeVisible();
    await page.getByRole('textbox', { name: 'Message to the agent' }).fill('Keep my unsent draft');
    const reads = calls.filter((call) => call.path.endsWith('/versions'));
    expect(reads).toEqual([]);
    await strip.getByRole('button', { name: 'Previous version' }).click();
    await expect(strip).toContainText('1/2');
    await expect(page.locator('[data-screen="thread"]').getByText('Original release plan', { exact: true })).toBeVisible();
    await expect(page.locator('[data-screen="thread"]').getByText('Revised release plan', { exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Back to active conversation', exact: true })).toBeVisible();
    await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toHaveCount(0);
    await expect(page).toHaveURL(new RegExp(`continuation=${A}`));
    expect(calls.filter((call) => call.method === 'POST' && !call.path.endsWith('/read'))).toEqual([]);
    expect(calls.filter((call) => call.path.endsWith('/versions'))).toHaveLength(1);
    await page.mouse.move(0, 0); await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: info.outputPath(`message-versions-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
    const bounds = await strip.boundingBox(); expect(bounds).not.toBeNull();
    expect(bounds!.x).toBeGreaterThanOrEqual(0); expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    await page.reload();
    await expect(strip).toContainText('1/2');
    await expect(page.locator('[data-screen="thread"]').getByText('Original release plan', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'Back to active conversation', exact: true }).click();
    await expect(strip).toContainText('2/2');
    await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Keep my unsent draft');
    expect(errors).toEqual([]);
  });
}

test('Edit is the same named right-click action and submits an exact new version', async ({ page }, info) => {
  await begin(); const { calls } = await versions(page, false); await openThread(page);
  const message = page.locator(`[data-item="${ITEM_A}"]`);
  await message.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Edit message', exact: true }).click();
  const editor = page.getByRole('textbox', { name: 'Edit message', exact: true });
  await expect(editor).toHaveValue('Original release plan');
  await editor.fill('Edited release plan for the first release');
  await page.screenshot({ path: info.outputPath('message-edit-form.png'), fullPage: true, animations: 'disabled' });
  await page.getByRole('button', { name: 'Send edit', exact: true }).click();
  await expect(page.getByRole('group', { name: 'Message versions', exact: true })).toContainText('2/2');
  await expect(page.locator('[data-screen="thread"]').getByText('Edited release plan for the first release', { exact: true })).toBeVisible();
  const writes = calls.filter((call) => call.method === 'POST' && call.path.endsWith('/versions'));
  expect(writes).toHaveLength(1);
  expect(writes[0]?.body).toMatchObject({ source_continuation_id: A, expected_active_continuation_id: A,
    expected_execution_revision: 1, expected_queue_revision: 8,
    content: [{ type: 'text', text: 'Edited release plan for the first release' }] });
  expect(writes[0]?.body).toHaveProperty('client_id');
  expect(writes[0]?.body).not.toHaveProperty('active_turn');
  await page.getByRole('button', { name: 'Previous version' }).click();
  await expect(page.locator('[data-screen="thread"]').getByText('Original release plan', { exact: true })).toBeVisible();
});

test('continuing the original version is explicit and never reruns its old prompt', async ({ page }) => {
  await begin(); const { calls } = await versions(page); await openThread(page);
  await page.getByRole('button', { name: 'Previous version' }).click();
  await expect(page.getByRole('group', { name: 'Message versions', exact: true })).toContainText('1/2');
  expect(calls.filter((call) => call.method === 'POST' && call.path.endsWith('/activate'))).toEqual([]);
  await page.getByRole('button', { name: 'Continue from this version', exact: true }).click();
  await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await expect(page).not.toHaveURL(/continuation=/);
  const writes = calls.filter((call) => call.method === 'POST' && !call.path.endsWith('/read'));
  expect(writes).toHaveLength(1);
  expect(writes[0]?.path).toBe(`/api/v1/threads/${MAIN}/continuations/${A}/activate`);
  expect(writes[0]?.body).toMatchObject({ expected_active_continuation_id: B, expected_execution_revision: 2, expected_queue_revision: 8 });
  expect(writes[0]?.body).not.toHaveProperty('content');
  expect(writes[0]?.body).not.toHaveProperty('active_turn');
});


for (const [width, theme] of [[1440, 'light'], [400, 'dark']] as const) {
  test(`failed-version pending edit and explicit retry at ${width} in ${theme}`, async ({ page }, info) => {
    await begin(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const { calls } = await versions(page, true, { failed: true });
    await openThread(page, undefined, `/thread/${MAIN}?continuation=${B}&prompt=${INPUT_B}`);
    const queue = page.locator('[data-queued]');
    await expect(page.getByText('No messages in this version yet', { exact: true })).toBeVisible();
    await expect(page.getByText('Send the first message', { exact: true })).toHaveCount(0);
    await expect(queue).toContainText('Revised release plan');
    await expect(queue.getByRole('combobox')).toHaveCount(0);
    await expect(queue.getByRole('button', { name: /Send now|Move.*earlier|Move.*later|Drag/i })).toHaveCount(0);
    await queue.getByRole('button', { name: /Edit/ }).click();
    const editor = queue.getByRole('textbox'); await editor.fill('Retained revised words');
    await queue.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(queue).toContainText('Retained revised words');
    const writes = calls.filter(call => call.method === 'PATCH'); expect(writes).toHaveLength(1);
    expect(writes[0]?.body).toEqual({ content: [{ type: 'text', text: 'Retained revised words' }] });
    expect(calls.some(call => call.path.endsWith('/activate'))).toBe(false);
    await page.reload(); await expect(queue).toContainText('Retained revised words');
    await page.mouse.move(0, 0); await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: info.outputPath(`failed-version-queue-${width}-${theme}.png`), fullPage: true, animations: 'disabled' });
    await page.locator('[data-continuation-notice]').getByRole('button', { name: 'Try again', exact: true }).click();
    await expect(page.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
    await expect(page).not.toHaveURL(/continuation=/);
    expect(calls.filter(call => call.path.endsWith('/activate'))).toHaveLength(1);
    expect(calls.find(call => call.path.endsWith('/activate'))?.body).not.toHaveProperty('content');
    expect(calls.filter(call => call.method === 'POST' && call.path.endsWith('/versions'))).toHaveLength(0);
  });
}

test('removing inactive pending words never resumes or activates the thread', async ({ page }) => {
  await begin(); const { calls } = await versions(page, true, { failed: true });
  await openThread(page, undefined, `/thread/${MAIN}?continuation=${B}`);
  const queue = page.locator('[data-queued]'); await expect(queue).toHaveCount(1);
  await queue.getByRole('button', { name: /Remove/ }).click(); await expect(queue).toHaveCount(0);
  expect(calls.filter(call => call.method === 'DELETE')).toHaveLength(1);
  expect(calls.some(call => call.path.endsWith('/activate') || call.path.endsWith('/resume'))).toBe(false);
});
