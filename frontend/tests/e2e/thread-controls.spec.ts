/** The compact header and full composer row at desktop and phone widths, with native-mode choices and references that wait in the draft. */
import { expect, type Locator, type Page, test } from '@playwright/test';
import type { ThreadSync } from '../../src/generated/api-types';

import { begin } from './inspector-support.ts';
import { ACME, emit, PEOPLE, playScene, signIn, threadHeld } from './support.ts';
import { turn } from '../doubles/thread-rows.ts';

const MAIN = ACME.mainThread;
const AT = '2026-10-15T00:00:00.000Z';
const controls = (page: Page) => page.getByRole('group', { name: 'Message controls', exact: true });
const box = (page: Page) => page.getByRole('textbox', { name: 'Message to the agent' });

test.beforeEach(async () => {
  await begin('');
  await emit(MAIN, { thread: { state: 'idle', description: '', usage: { input_tokens: 12_000, cached_input_tokens: 0, output_tokens: 3_000, reasoning_tokens: 0, total_tokens: 15_000,
    context_used: 142_000, context_window: 200_000, compact_at: 180_000, updated_at: AT } } });
});

async function open(page: Page): Promise<void> {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/thread/${MAIN}`);
  await expect(box(page)).toBeVisible();
  await expect(controls(page).getByRole('button', { name: 'Approval mode: On request' })).toBeVisible();
}

async function bounds(locator: Locator) {
  await expect(locator).toBeVisible();
  const rect = await locator.boundingBox();
  expect(rect).not.toBeNull();
  return rect!;
}

/** Check painted text, rather than textContent alone: an ellipsis keeps the missing words in the DOM. */
async function fullyShown(locator: Locator): Promise<void> {
  await expect(locator).toBeVisible();
  const text = await locator.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(element);
    return {
      box: { left: box.left, right: box.right, top: box.top, bottom: box.bottom },
      lines: Array.from(range.getClientRects(), (line) => ({ left: line.left, right: line.right, top: line.top, bottom: line.bottom })),
    };
  });
  expect(text.lines.length).toBeGreaterThan(0);
  for (const line of text.lines) {
    expect(line.left).toBeGreaterThanOrEqual(text.box.left - 1);
    expect(line.right).toBeLessThanOrEqual(text.box.right + 1);
    expect(line.top).toBeGreaterThanOrEqual(text.box.top - 1);
    expect(line.bottom).toBeLessThanOrEqual(text.box.bottom + 1);
  }
}

for (const width of [1280, 1440, 390, 400]) {
  test(`the header and every message control fit with readable compact labels at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await open(page);
    const header = page.locator('[data-thread-header]');
    const quick = header.getByRole('group', { name: 'Thread actions', exact: true });
    for (const button of await quick.getByRole('button').all()) {
      const rect = await bounds(button);
      const frame = await bounds(header);
      // Frequent acts are icons on one compact row, including on the phone.
      expect(rect.height).toBeLessThanOrEqual(29);
      expect(rect.x).toBeGreaterThanOrEqual(frame.x - 1);
      expect(rect.x + rect.width).toBeLessThanOrEqual(Math.min(width, frame.x + frame.width) + 1);
    }
    const title = await bounds(header.getByRole('heading', { name: 'Main thread' }));
    const state = await bounds(header.getByText('Idle', { exact: true }));
    expect(state.x).toBeGreaterThanOrEqual(title.x + title.width);
    const actionRow = await bounds(header.locator('[data-thread-actions-row]'));
    if (width < 640) expect(actionRow.y).toBeGreaterThanOrEqual(title.y + title.height);
    const actionCenters: number[] = [];
    // Fractional text metrics and icon-button padding may round differently by a pixel.
    for (const action of ['Star', 'Notifications for this thread', 'Share', 'Actions for Main thread']) {
      const button = await bounds(header.getByRole('button', { name: action, exact: true }));
      actionCenters.push(button.y + button.height / 2);
      if (width >= 640) expect(Math.abs(button.y + button.height / 2 - state.y - state.height / 2)).toBeLessThanOrEqual(2);
      expect(button.x + button.width).toBeLessThanOrEqual(width);
    }
    for (const name of ['Fork', 'Fork to…', 'Ask on the side…', 'Switch provider or agent…', 'Find in the conversation']) {
      const button = quick.getByRole('button', { name, exact: true });
      const rect = await bounds(button);
      actionCenters.push(rect.y + rect.height / 2);
      await expect(button).toHaveText('');
    }
    expect(Math.max(...actionCenters) - Math.min(...actionCenters)).toBeLessThanOrEqual(2);
    await expect(header.getByText('Add a description')).toHaveCount(0);
    await expect(header.getByRole('button', { name: /Model and effort|Approval mode|Show what fills the context|Edit links/ })).toHaveCount(0);
    const row = controls(page);
    const names = await row.getByRole('button').evaluateAll((buttons) => buttons.map((button) => button.getAttribute('aria-label') ?? button.textContent.trim()));
    expect(names[0]).toBe('Add to message');
    expect(names[1]).toMatch(/^Agent:/);
    expect(names[2]).toMatch(/^Model and effort:/);
    expect(names[3]).toBe('Approval mode: On request');
    expect(names[4]).toMatch(/^Show what fills the context/);
    expect(names.slice(5)).toEqual(['Send', 'More ways to send']);
    const add = await bounds(row.getByRole('button', { name: 'Add to message' }));
    const mode = await bounds(row.getByRole('button', { name: 'Approval mode: On request' }));
    const ring = await bounds(row.getByRole('button', { name: /^Show what fills the context/ }));
    const send = await bounds(row.getByRole('button', { name: 'Send', exact: true }));
    const more = await bounds(row.getByRole('button', { name: 'More ways to send' }));
    const rowBox = await bounds(row);
    expect(send.height).toBe(mode.height);
    expect(more.height).toBe(mode.height);
    expect(send.width).toBeLessThan(rowBox.width / 2);
    expect(send.x).toBeGreaterThan(ring.x);
    expect(more.x + more.width).toBeLessThanOrEqual(Math.min(width, rowBox.x + rowBox.width) + 1);
    expect(send.y + send.height).toBeLessThanOrEqual(rowBox.y + rowBox.height + 1);
    expect(send.y + send.height).toBeLessThanOrEqual(900);
    expect(send.x).toBeGreaterThanOrEqual(rowBox.x);
    expect(Math.abs(add.y - send.y)).toBeLessThanOrEqual(1);
    expect(Math.abs(mode.y + mode.height / 2 - send.y - send.height / 2)).toBeLessThanOrEqual(2);
    await expect(box(page)).toHaveCSS('resize', 'none');
    if (width < 640) await expect(page.locator('[data-mobile-main] + nav')).toHaveCount(0);
    await expect(row.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
    await box(page).fill('Read these files first');
    await expect(row.getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
  });
}

for (const width of [1440, 400]) {
  test(`running delivery uses the same ordinary control height and stays at the right end at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await emit(MAIN, { thread: { state: 'running' }, turns: [turn('control-turn', { ordinal: 1, state: 'running', inputId: 'control-input' })] });
    await open(page);
    const row = controls(page);
    const stop = await bounds(row.getByRole('button', { name: 'Stop', exact: true }));
    const mode = await bounds(row.getByRole('button', { name: 'Approval mode: On request' }));
    const more = await bounds(row.getByRole('button', { name: 'More ways to send' }));
    expect(stop.height).toBe(mode.height);
    expect(more.height).toBe(mode.height);
    expect(more.x + more.width).toBeLessThanOrEqual(width);
    expect(stop.y + stop.height).toBeLessThanOrEqual(900);
    expect(Math.abs(stop.y - mode.y)).toBeLessThanOrEqual(1);
    await row.getByRole('button', { name: 'More ways to send' }).click();
    await expect(page.getByRole('menuitem', { name: /^Queue/ })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: /^Steer now/ })).toBeVisible();
  });

  test(`a running sub-agent opens its indented lane and the same read-only panel at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await playScene(MAIN, 'everything');
    await open(page);
    await page.getByRole('button', { name: "Show reviewer's activity" }).click();
    const lane = page.getByRole('region', { name: 'Sub-agent reviewer', exact: true });
    await expect(lane.getByRole('log')).toContainText('The redirect needs to stay on the same origin.');
    await expect(lane).toContainText('Role: review');
    const border = await lane.evaluate((element) => ({ width: getComputedStyle(element).borderLeftWidth, padding: getComputedStyle(element).paddingLeft }));
    expect(Number.parseFloat(border.width)).toBeGreaterThan(0);
    expect(Number.parseFloat(border.padding)).toBeGreaterThan(0);
    await expect(lane.getByRole('button', { name: 'Open in panel' })).toHaveCount(0);
    const parentRow = page.locator('[data-step="t-sub"] [data-tool-row]').first();
    const raw = parentRow.getByRole('button', { name: 'Show the raw call' });
    const openPanel = parentRow.getByRole('button', { name: 'Open in panel' });
    for (const action of [raw, openPanel]) {
      const actionBox = await bounds(action), rowBox = await bounds(parentRow);
      expect(Math.abs(actionBox.y + actionBox.height / 2 - rowBox.y - rowBox.height / 2)).toBeLessThanOrEqual(1);
      await expect(action).toHaveText('');
    }
    await openPanel.click();
    const panel = page.locator('[data-subagent-panel="t-sub"]');
    await expect(panel.getByRole('log')).toContainText('The redirect needs to stay on the same origin.');
    await expect(panel.getByRole('button', { name: /npm test login/ })).toBeVisible();
    await expect(panel.getByRole('textbox')).toHaveCount(0);
  });
}

test('at 400 px a long title keeps every word beside its state, above one action row', async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 900 });
  await open(page);
  const name = 'New thread about the release and the final provider review';
  await emit(MAIN, { thread: { name } });
  const header = page.locator('[data-thread-header]');
  const heading = header.getByRole('heading', { name, exact: true });
  await fullyShown(heading);
  const title = await bounds(heading);
  const state = await bounds(header.getByText('Idle', { exact: true }));
  expect(title.x + title.width).toBeLessThanOrEqual(400);
  expect(state.x).toBeGreaterThanOrEqual(title.x + title.width);
  expect((await bounds(header.locator('[data-thread-actions-row]'))).y).toBeGreaterThanOrEqual(title.y + title.height);
  await expect(header.getByRole('button', { name: 'Share', exact: true })).toBeVisible();
  await header.getByRole('button', { name: /Rename/ }).click();
  await expect(header.getByRole('textbox')).toHaveValue(name);
});

test('at 400 px full agent and long model names stay available without enlarging Send’s row', async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 900 });
  await open(page);
  const response = await page.request.get(`/api/v1/threads/${MAIN}/sync`);
  expect(response.ok()).toBe(true);
  const { thread } = await response.json() as ThreadSync;
  if (thread.binding === null) throw new Error('The main fixture needs its real agent binding');
  const agent = 'Scripted agent for the release review';
  const model = 'Mock Large reasoning model for release work';
  await emit(MAIN, { thread: { binding: { ...thread.binding, agent: { ...thread.binding.agent, name: agent } }, model_display: model, effort: 'high' } });
  const row = controls(page);
  const agentButton = row.getByRole('button', { name: `Agent: ${agent}. Change binding`, exact: true });
  await expect(agentButton.locator('span').first()).toHaveText(agent);
  await expect(agentButton.locator('span').first()).not.toHaveClass(/\bhidden\b/);
  await agentButton.hover();
  await expect(page.getByRole('tooltip')).toContainText(agent);
  await agentButton.focus();
  await expect(page.getByRole('tooltip')).toContainText(agent);
  const modelButton = row.getByRole('button', { name: /^Model and effort: Mock Large reasoning model for release work/ });
  await expect(modelButton).toContainText(model);
  await modelButton.hover();
  await expect(page.getByRole('tooltip')).toContainText(model);
  await modelButton.focus();
  await expect(page.getByRole('tooltip')).toContainText(model);
  for (const button of [agentButton, modelButton, row.getByRole('button', { name: 'More ways to send' })]) {
    const rect = await bounds(button);
    expect(rect.x).toBeGreaterThanOrEqual(0);
    expect(rect.x + rect.width).toBeLessThanOrEqual(400);
  }
  await box(page).fill('Read the complete names before sending');
  const send = row.getByRole('button', { name: 'Send', exact: true });
  await expect(send).toBeEnabled();
  const sendBox = await bounds(send);
  expect(sendBox.height).toBe((await bounds(modelButton)).height);
  expect(sendBox.height).toBe((await bounds(agentButton)).height);
  expect(Math.abs(sendBox.y - (await bounds(modelButton)).y)).toBeLessThanOrEqual(1);
  expect(sendBox.y + sendBox.height).toBeLessThanOrEqual(900);
});

for (const width of [400, 1440]) {
  test(`the blocked reason has room and its action fits at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await open(page);
    const detail = 'The selected sandbox is unavailable on this machine. Choose another agent or change the environment settings.';
    await emit(MAIN, { thread: { state: 'blocked', blocked: { reason: 'binding_invalid', detail } } });
    const reason = page.locator('[data-blocked="binding_invalid"]');
    await expect(reason).toContainText(detail);
    const banner = reason.locator('..').locator('..');
    const action = banner.getByRole('button', { name: /Switch.*(provider|agent)/ });
    const text = await bounds(reason.locator('..'));
    const button = await bounds(action);
    const alert = await bounds(banner);
    expect(button.x + button.width).toBeLessThanOrEqual(alert.x + alert.width);
    if (width < 640) {
      expect(button.y).toBeGreaterThanOrEqual(text.y + text.height);
      // After the notice icon and padding, the reason gets the rest of the phone's line.
      expect(text.width).toBeGreaterThan(alert.width * 0.75);
    } else expect(button.x).toBeGreaterThanOrEqual(text.x + text.width);
    await action.click();
    await expect(page.getByRole('dialog', { name: 'Switch “Main thread”', exact: true })).toBeVisible();
  });
}

for (const width of [400, 1440]) {
  test(`item actions share the first text line's centre at ${width}px in both themes`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await playScene(MAIN, 'everything');
    await open(page);
    const log = page.getByRole('log', { name: 'Conversation', exact: true });
    await expect(log.locator('[data-item-acts]').first()).toBeAttached();
    for (const theme of ['light', 'dark'] as const) {
      await page.emulateMedia({ colorScheme: theme });
      await page.evaluate(() => document.fonts.ready);
      await expect.poll(async () => log.locator('[data-item]').evaluateAll((items) => {
        const offsets: number[] = [];
        for (const item of items) {
          const menu = item.querySelector(':scope > [data-item-acts] button');
          const body = item.querySelector(':scope > [data-item-body]');
          if (menu === null || body === null || menu.getBoundingClientRect().height === 0) continue;
          const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
          for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
            if (node.textContent?.trim() === '' || node.parentElement?.closest('[aria-hidden="true"], [role="img"], svg, .avatar, [hidden], .sr-only') !== null) continue;
            const range = document.createRange(); range.selectNodeContents(node);
            const line = range.getClientRects()[0];
            if (line === undefined || line.width === 0 || line.height <= 1) continue;
            const button = menu.getBoundingClientRect();
            offsets.push(Math.abs(button.top + button.height / 2 - line.top - line.height / 2));
            break;
          }
        }
        return offsets.length === 0 ? 1000 : Math.max(...offsets);
      })).toBeLessThan(1);
    }
  });
}

test('the composer approval picker changes the displayed native mode without inventing Auto support', async ({ page }) => {
  await open(page);
  await controls(page).getByRole('button', { name: 'Approval mode: On request' }).click();
  const modes = page.getByRole('dialog', { name: 'Approval mode', exact: true });
  await expect(modes.getByRole('button', { name: 'Ask', exact: true })).toBeEnabled();
  await expect(modes.getByRole('button', { name: 'Auto', exact: true })).toHaveCount(0);
  await modes.getByRole('button', { name: 'Ask', exact: true }).click();
  await expect(controls(page).getByRole('button', { name: 'Approval mode: Ask' })).toBeVisible();
  expect((await threadHeld(MAIN)).inputs).toHaveLength(0);
});

test('plus, /file and @ reach the same file picker and add actual paths to an unsent message', async ({ page }) => {
  await open(page);
  const row = controls(page);
  await row.getByRole('button', { name: 'Add to message' }).click();
  await expect(page.getByRole('menuitem', { name: 'Attach files…' })).toBeVisible();
  await expect(page.getByRole('menuitem', { name: 'Mention a document…' })).toBeVisible();
  await page.getByRole('menuitem', { name: 'Mention a workspace file…' }).click();
  const picker = page.getByRole('dialog', { name: 'Find a file or document' });
  await picker.getByRole('button', { name: 'README.md', exact: true }).click();
  await expect(box(page)).toHaveValue('`README.md`\n');
  expect((await threadHeld(MAIN)).inputs).toHaveLength(0);

  await box(page).fill('/file app.ts');
  await box(page).press('Enter');
  await picker.getByRole('button', { name: 'web/src/app.ts', exact: true }).click();
  await expect(box(page)).toHaveValue('`web/src/app.ts`\n');
  await box(page).fill('');
  await box(page).press('@');
  await expect(picker).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(box(page)).toHaveValue('');
  expect((await threadHeld(MAIN)).inputs).toHaveLength(0);
});

test('a chosen document is attached as a copy and still waits for Send', async ({ page }) => {
  await open(page);
  await box(page).fill('/document Release checklist');
  await box(page).press('Enter');
  const picker = page.getByRole('dialog', { name: 'Find a file or document' });
  await picker.getByRole('button', { name: /^Release checklist/ }).click();
  await expect(picker).toHaveCount(0);
  const attachments = page.getByRole('list', { name: 'Attached files' });
  await expect(attachments).toContainText('Release checklist');
  await expect(controls(page).getByRole('button', { name: 'Send', exact: true })).toBeEnabled();
  expect((await threadHeld(MAIN)).inputs).toHaveLength(0);
});
