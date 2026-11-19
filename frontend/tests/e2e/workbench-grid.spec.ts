/** Real production browser, ordinary native-shaped fake thread transport; no model execution claim. */
import { expect, test, type Page } from '@playwright/test';
import { ACME, changeTree, configureThread, emit, scenario, signIn, threadHeld } from './support';
import { agentMessage, marker, turn } from '../doubles/thread-rows';
import type { ThreadInstructions } from '../../src/generated/api-types';
import { showWorkbenchView } from './workbench-support';

const ids = Array.from({ length: 160 }, (_, index) => `01960000-0000-7000-8000-${String(index + 1).padStart(12, '0')}`);
const tabId = (index: number) => `grid-content-${index}`;
const pane = (page: Page, index: number) => page.locator(`[data-view-instance="${tabId(index)}"]`);
async function start(page: Page, count: number) {
  await scenario();
  await changeTree(ids.slice(0, count).map((id, index) => ({ id, kind: 'thread', name: `Grid agent ${index + 1}`, parent_id: ACME.fixLogin,
    thread: { state: 'idle', blocked_reason: null, pending_approvals: 0, activity_seq: 1, last_activity_at: null } })));
  for (const id of ids.slice(0, count)) { await configureThread(id, { reset: true }); await emit(id, { thread: { state: 'idle', directory: 'web' } }); }
  await signIn(page);
  await expect(page.getByRole('button', { name: 'Layout', exact: true }).first()).toBeVisible();
  await expect.poll(() => page.evaluate(() => Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).some((key) => key?.endsWith('.workbench.workspace')))).toBe(true);
  // Seed before app startup: the running page correctly flushes its own state on unload.
  const prepared = await page.evaluate(({ ids, tabs }) => {
    const key = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).find((key) => key?.endsWith('.workbench.workspace'));
    if (key === null || key === undefined) throw new Error('The signed-in workspace was not persisted');
    const saved = JSON.parse(localStorage.getItem(key) ?? 'null') as { layout: string; [key: string]: unknown };
    const layout = JSON.parse(saved.layout) as { main: unknown; [key: string]: unknown };
    layout.main = { kind: 'group', id: 'grid-origin', tabs: ids.map((id, index) => ({ id: tabs[index], view: 'page', of: { href: `/thread/${id}` } })), active: tabs[0] };
    return { key, value: JSON.stringify({ ...saved, layout: JSON.stringify(layout), focusedGroup: 'grid-origin', focusedTab: tabs[0], focusedMainGroup: 'grid-origin', focusedMainTab: tabs[0], activeContent: tabs[0], focusMode: false, closedTabs: [] }) };
  }, { ids: ids.slice(0, count), tabs: ids.slice(0, count).map((_, index) => tabId(index)) });
  await page.addInitScript(({ key, value }) => {
    if (sessionStorage.getItem('grid-fixture-loaded') === 'yes') return;
    localStorage.setItem(key, value); sessionStorage.setItem('grid-fixture-loaded', 'yes');
  }, prepared);
  await page.goto(`/thread/${ids[0]}`);
  await expect(pane(page, 0).getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
}
async function layoutAction(page: Page, label: string, check = false) {
  await page.getByRole('button', { name: 'Layout', exact: true }).first().click();
  await page.getByRole(check ? 'menuitemcheckbox' : 'menuitem', { name: label, exact: true }).click();
}
async function controlsFit(page: Page) {
  // Small cells may scroll their full view. Its normal composer controls must be reachable
  // through that actual scroll area, without changing layout, hiding controls or forcing a click.
  for (const view of await page.locator('[data-view-instance]:has([data-screen="thread"])').all()) {
    await view.locator('[data-composer-controls] button').last().scrollIntoViewIfNeeded();
  }
  const escaped = await page.locator('[data-view-instance]:has([data-screen="thread"])').evaluateAll((views: Element[]): string[] => {
      const outside: string[] = [];
      for (const view of views) {
        const bounds = view.getBoundingClientRect();
        for (const control of Array.from(view.querySelectorAll('[data-composer-controls] button'))) {
          if (!(control instanceof HTMLElement)) continue;
          const rect = control.getBoundingClientRect();
          if (rect.width > 0 && (rect.left < bounds.left - 1 || rect.right > bounds.right + 1 || rect.top < bounds.top - 1 || rect.bottom > bounds.bottom + 1)) outside.push(control.getAttribute('aria-label') ?? control.textContent);
        }
      }
      return outside;
    });
  expect(escaped).toEqual([]);
}
for (const [count, grid] of [[8, '2 rows × 4 columns'], [9, '3 rows × 3 columns']] as const) for (const theme of ['light', 'dark'] as const) {
  test(`${count} full thread panes retain correct Send/Stop targets in ${theme}`, async ({ page }, info) => {
    await page.setViewportSize({ width: 1440, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const errors: string[] = [], writes: string[] = []; let sockets = 0;
    page.on('pageerror', (error) => errors.push(error.message));
    page.on('websocket', () => { sockets += 1; });
    page.on('request', (request) => {
      if (request.method() !== 'GET' && /\/threads\/[^/]+\/(?:inputs|interrupt|resume|continuations|binding)(?:\/|$|\?)/.test(new URL(request.url()).pathname)) writes.push(new URL(request.url()).pathname);
    });
    await start(page, count);
    const connected = sockets;
    await layoutAction(page, grid);
    await expect(page.locator('[data-screen="thread"]:visible')).toHaveCount(count);
    for (let index = 0; index < count; index += 1) await expect(pane(page, index).getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
    await page.screenshot({ path: info.outputPath(`grid-${count}-${theme}-frame.png`), animations: 'disabled' });
    expect(writes).toEqual([]);
    await controlsFit(page);
    await page.screenshot({ path: info.outputPath(`grid-${count}-${theme}-frame-controls.png`), animations: 'disabled' });
    await layoutAction(page, 'Focus mode', true);
    await expect(page.getByRole('navigation', { name: 'Orgs and your own places' })).toHaveCount(0);
    await expect(page.locator('[data-workbench-chrome]')).toHaveCount(0);
    await expect(page.locator('[data-workbench-bar]')).toHaveCount(0);
    for (let index = 0; index < count; index += 1) {
      const view = pane(page, index);
      await view.getByRole('textbox', { name: 'Message to the agent' }).fill(`Message for agent ${index + 1}`);
      await view.getByRole('button', { name: 'Send', exact: true }).click();
      await expect.poll(async () => (await threadHeld(ids[index]!)).inputs.map((input) => input.content)).toEqual([[{ type: 'text', text: `Message for agent ${index + 1}` }]]);
      const live = turn(`01960000-0000-7000-8001-${String(index + 1).padStart(12, '0')}`, { ordinal: 101, state: 'running', inputId: (await threadHeld(ids[index]!)).inputs[0]!.id, tokens: 10_000_000_000 });
      await emit(ids[index]!, { turns: [{ ...live, thread_id: ids[index]! }], thread: { state: 'running', current_turn_id: live.id } });
      await expect(view.getByRole('button', { name: 'Stop', exact: true })).toBeVisible();
      await view.getByRole('button', { name: 'Stop', exact: true }).click();
      await expect.poll(async () => (await threadHeld(ids[index]!)).turns.find((entry) => entry.id === live.id)?.state).toBe('interrupted');
    }
    await page.screenshot({ path: info.outputPath(`grid-${count}-${theme}-focus.png`), animations: 'disabled' });
    expect(writes.filter((path) => path.endsWith('/inputs'))).toEqual(ids.slice(0, count).map((id) => `/api/v1/threads/${id}/inputs`));
    expect(writes.filter((path) => path.endsWith('/interrupt'))).toEqual(ids.slice(0, count).map((id) => `/api/v1/threads/${id}/interrupt`));
    expect(writes.filter((path) => !path.endsWith('/inputs') && !path.endsWith('/interrupt'))).toEqual([]);
    expect(sockets).toBe(connected);
    await controlsFit(page);
    expect(errors).toEqual([]);
  });
}

test('close-all, reload, reopen and reset retain the draft and a usable workbench entrance', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 2);
  await pane(page, 0).getByRole('textbox', { name: 'Message to the agent' }).fill('Recover this ordinary conversation draft');
  await layoutAction(page, '3 rows × 3 columns');
  await layoutAction(page, 'Close all main tabs');
  await expect(page).toHaveURL(/\/workbench\/?$/);
  await expect(page.getByRole('button', { name: 'Open…', exact: true })).toHaveCount(9);
  await page.reload();
  await expect(page).toHaveURL(/\/workbench\/?$/);
  await expect(page.getByRole('button', { name: 'Open…', exact: true })).toHaveCount(9);
  await layoutAction(page, 'Reopen closed tab');
  await layoutAction(page, 'Reopen closed tab');
  await expect(pane(page, 0).getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Recover this ordinary conversation draft');
  await layoutAction(page, 'Reset layout');
  await expect(page.locator('[data-main-area] [role="tablist"]')).toHaveCount(1);
  await expect(page.locator('[data-main-area] [role="tab"]')).toHaveCount(2);
  await expect(pane(page, 0).getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Recover this ordinary conversation draft');
});


test('leaving focus mode on a desktop brings the bars back as the layout has them', async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 1);
  const left = page.locator('[data-workbench-bar="left"]');
  const toggle = page.getByRole('button', { name: 'Left bar', exact: true }).first();
  // The left bar is closed when the frame is first drawn and opened after it, so the size it has then is not the one the
  // frame began with: leaving focus mode must bring it back open, at that size.
  if (await left.isVisible()) await toggle.click();
  await expect(left).toHaveCount(0);
  await page.reload();
  await expect(page.getByRole('button', { name: 'Layout', exact: true }).first()).toBeVisible();
  await expect(left).toHaveCount(0);
  await toggle.click();
  await expect(left).toBeVisible();
  const before = (await left.boundingBox())?.width ?? 0;
  expect(before).toBeGreaterThan(0);
  await layoutAction(page, 'Focus mode', true);
  await expect(page.locator('[data-workbench-bar]')).toHaveCount(0);
  await page.locator('[data-main-area] [role="tab"]').first().click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Exit focus mode', exact: true }).click();
  await expect(page.locator('[data-workbench-chrome]')).toBeVisible();
  await expect(left).toBeVisible();
  expect(Math.round((await left.boundingBox())?.width ?? 0)).toBe(Math.round(before));
});

async function savedMain(page: Page): Promise<unknown> {
  return page.evaluate(() => {
    const key = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).find((key) => key?.endsWith('.workbench.workspace'));
    if (key === null || key === undefined) throw new Error('No signed-in workspace');
    const saved = JSON.parse(localStorage.getItem(key) ?? 'null') as { layout: string };
    return (JSON.parse(saved.layout) as { main: unknown }).main;
  });
}

test('phone presentation retains the nine-cell desktop arrangement and offers every main tab in focus mode', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 9);
  await layoutAction(page, '3 rows × 3 columns');
  await expect(page.locator('[data-screen="thread"]:visible')).toHaveCount(9);
  const before = await savedMain(page);
  await page.setViewportSize({ width: 400, height: 900 });
  await expect(page.locator('[data-mobile-main-tabs]')).toBeVisible();
  await expect(pane(page, 0).getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await page.getByRole('button', { name: 'Find tab in Main area', exact: true }).click();
  await page.locator(`[role="option"][data-value="commands:workspace.focus.${tabId(8)}"]`).click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[8]}$`));
  await expect(pane(page, 8).getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await layoutAction(page, 'Focus mode', true);
  await expect(page.locator('[data-workbench-chrome], [data-navigation-rail]')).toHaveCount(0);
  await expect(page.locator('[data-screen="thread"]:visible')).toHaveCount(1);
  await page.screenshot({ path: info.outputPath('grid-nine-phone-focus.png'), animations: 'disabled' });
  await page.locator('[data-mobile-main-tabs] [role="tab"][aria-selected="true"]').click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Exit focus mode', exact: true }).click();
  await expect(page.locator('[data-workbench-chrome]')).toBeVisible();
  await page.setViewportSize({ width: 1440, height: 900 });
  await expect(page.locator('[data-screen="thread"]:visible')).toHaveCount(9);
  expect(await savedMain(page)).toEqual(before);
});

test('detached Files stays on A while B is focused and after A conversation closes', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 2);
  await showWorkbenchView(page, 'Files');
  const section = page.locator('[data-workbench-bar="right"]').getByRole('button', { name: 'Code actions', exact: true });
  await expect(page.getByTestId('files-panel')).toBeVisible();
  await section.click();
  await page.getByRole('menuitem', { name: 'Move to', exact: true }).hover();
  const split = page.getByRole('menuitem', { name: 'Split right', exact: true });
  await expect(split).toBeVisible();
  await info.attach('split-menu-before-pointer.json', { contentType: 'application/json', body: JSON.stringify(await split.evaluate((item) => {
    const rect = item.getBoundingClientRect(); const menu = item.closest('[role=menu]'); const top = document.elementFromPoint(rect.x + rect.width / 2, rect.y + rect.height / 2);
    return { item: { x: rect.x, y: rect.y, width: rect.width, height: rect.height }, menu: menu === null ? null : { x: menu.getBoundingClientRect().x, y: menu.getBoundingClientRect().y, width: menu.clientWidth, height: menu.clientHeight }, hit: top?.outerHTML.slice(0, 600), side: menu?.getAttribute('data-side'), styles: { pointer: getComputedStyle(item).pointerEvents, overflow: menu === null ? null : getComputedStyle(menu).overflowY } };
  })) });
  const destination = await split.boundingBox();
  if (destination === null) throw new Error('Split destination is not visible');
  await page.evaluate(() => {
    const trail: unknown[] = [];
    document.addEventListener('pointermove', (event) => {
      if (trail.length < 40) trail.push({ x: event.clientX, y: event.clientY, target: event.target instanceof Element ? event.target.closest('[role=menuitem], [role=menuitemcheckbox]')?.textContent : null, menus: Array.from(document.querySelectorAll('[role=menu]')).map((menu) => ({ side: menu.getAttribute('data-side'), open: menu.getAttribute('data-state') })) });
      document.documentElement.dataset.menuPointerProof = JSON.stringify(trail);
    }, { capture: true });
  });
  // Real pointer travel supplies intermediate directions to Radix's submenu grace area.
  await page.mouse.move(destination.x + destination.width / 2, destination.y + destination.height / 2, { steps: 20 });
  await info.attach('split-menu-pointer-path.json', { contentType: 'application/json', body: await page.locator('html').getAttribute('data-menu-pointer-proof') ?? '[]' });
  await split.click({ timeout: 15_000 });
  const fixed = page.getByRole('tab', { name: 'Files · Grid agent 1', exact: true });
  await expect(fixed).toBeVisible();
  await page.getByRole('tab', { name: 'Grid agent 2', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[1]}$`));
  await expect(fixed).toBeVisible();
  await expect(page.getByTestId('files-panel')).toContainText('web/');
  await page.getByRole('tab', { name: 'Grid agent 1', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'Grid agent 1', exact: true })).toHaveCount(0);
  await expect(fixed).toBeVisible();
  await expect(page.getByTestId('files-panel')).toBeVisible();
  const scope = await page.evaluate(() => {
    const key = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)).find((key) => key?.endsWith('.workbench.workspace'));
    const saved = JSON.parse(localStorage.getItem(key ?? '') ?? 'null') as { layout: string };
    const layout: unknown = JSON.parse(saved.layout);
    const find = (value: unknown): unknown => {
      if (typeof value !== 'object' || value === null) return null;
      if ('view' in value && value.view === 'files' && 'scope' in value) return value.scope;
      for (const child of Object.values(value)) { const found = find(child); if (found !== null) return found; }
      return null;
    };
    return find(layout);
  });
  expect(scope).toEqual({ mode: 'fixed', target: { kind: 'thread', id: ids[0] }, continuationId: null });
  await page.screenshot({ path: info.outputPath('fixed-files-after-conversation-close.png'), animations: 'disabled' });
  await page.reload();
  await expect(fixed).toBeVisible();
  await expect(page.getByTestId('files-panel')).toBeVisible();
});

test('duplicate full thread views share the draft, retain independent reading positions and keep copyable focused URLs', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 2);
  const first = pane(page, 0), firstTab = page.locator(`#tab-${tabId(0)}`);
  await first.getByRole('textbox', { name: 'Message to the agent' }).fill('Shared draft for this thread');
  await firstTab.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Open beside', exact: true }).click();
  await expect(page.locator('[data-screen="thread"]:visible')).toHaveCount(2);
  const duplicateId = await page.locator(`[data-view-instance]:not([data-view-instance="${tabId(0)}"]):has([data-screen="thread"])`).getAttribute('data-view-instance');
  if (duplicateId === null) throw new Error('The duplicate view has no stable identity');
  const duplicate = page.locator(`[data-view-instance="${duplicateId}"]`);
  await expect(duplicate.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Shared draft for this thread');
  await duplicate.getByRole('textbox', { name: 'Message to the agent' }).fill('Edited in the second view');
  await expect(first.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Edited in the second view');
  await page.getByRole('tab', { name: 'Grid agent 2', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[1]}$`));
  await firstTab.click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[0]}$`));
  await firstTab.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Copy link', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(new URL(`/thread/${ids[0]}`, page.url()).href);
  await layoutAction(page, 'Focus mode', true);
  const items = Array.from({ length: 160 }, (_, index) => ({ ...agentMessage(`grid-history-${index}`, `History entry ${index}. ${'Readable retained content. '.repeat(12)}`), thread_id: ids[0]! }));
  await emit(ids[0]!, { items });
  const left = first.getByRole('log', { name: 'Conversation', exact: true });
  const right = duplicate.getByRole('log', { name: 'Conversation', exact: true });
  await expect(left).toBeVisible(); await expect(right).toBeVisible();
  await expect(left).not.toHaveAttribute('aria-busy', 'true'); await expect(right).not.toHaveAttribute('aria-busy', 'true');
  await left.hover(); await page.mouse.wheel(0, -2400);
  await expect.poll(() => left.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight)).toBeGreaterThan(1000);
  const anchor = await left.evaluate((element) => {
    const top = element.getBoundingClientRect().top;
    const row = Array.from(element.querySelectorAll<HTMLElement>('[data-item]')).find((row) => row.getBoundingClientRect().bottom > top);
    if (row === undefined) throw new Error('No visible reading anchor');
    return { id: row.dataset.item, offset: row.getBoundingClientRect().top - top };
  });
  expect(await right.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight)).toBeLessThan(100);
  await firstTab.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
  await expect(first).toHaveCount(0); await expect(right).toBeVisible();
  await page.getByRole('tab', { name: 'Grid agent 1', exact: true }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Reopen closed tab', exact: true }).click();
  await expect(left).toBeVisible(); await expect(left).not.toHaveAttribute('aria-busy', 'true');
  const restored = left.locator(`[data-item="${anchor.id}"]`); await expect(restored).toBeVisible();
  await expect.poll(async () => Math.abs((await restored.evaluate((row) => row.getBoundingClientRect().top - (row.closest('[role="log"]')?.getBoundingClientRect().top ?? 0))) - anchor.offset)).toBeLessThan(8);
  await expect(first.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Edited in the second view');
  await page.reload(); await expect(left).toBeVisible(); await expect(restored).toBeVisible();
  await expect(first.getByRole('textbox', { name: 'Message to the agent' })).toHaveValue('Edited in the second view');
  await page.screenshot({ path: info.outputPath('duplicate-thread-recovered.png'), animations: 'disabled' });
});

test('a full chat keeps its address and composer as an ordinary closable main tab', async ({ page, context }, info) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write']);
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 2);
  await page.goto(`/place-chat/${ACME.fixLogin}`);
  await expect(page).toHaveURL(/\/chat\/[^/?]+$/);
  const href = page.url();
  const main = page.locator('[data-main-area]');
  const chat = main.locator('[data-view-instance]:has([data-chat-composer])');
  await expect(chat.getByRole('textbox', { name: /^Message / })).toBeVisible();
  await chat.getByRole('textbox', { name: /^Message / }).fill('A recoverable full-chat draft');
  await expect(main.getByRole('tab')).toHaveCount(3);
  const active = main.getByRole('tab', { selected: true });
  await active.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Copy link', exact: true }).click();
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(href);
  await main.getByRole('tab', { name: 'Grid agent 1', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[0]}$`));
  await main.getByRole('tab', { name: 'Fix login', exact: true }).click();
  await expect(page).toHaveURL(href);
  await expect(chat.getByRole('textbox', { name: /^Message / })).toHaveValue('A recoverable full-chat draft');
  await page.reload(); await expect(page).toHaveURL(href);
  await expect(chat.getByRole('textbox', { name: /^Message / })).toHaveValue('A recoverable full-chat draft');
  await active.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Close', exact: true }).click();
  await expect(main.getByRole('tab')).toHaveCount(2);
  await layoutAction(page, 'Reopen closed tab');
  await expect(page).toHaveURL(href); await expect(chat.getByRole('textbox', { name: /^Message / })).toHaveValue('A recoverable full-chat draft');
  await page.screenshot({ path: info.outputPath('full-chat-reopened.png'), animations: 'disabled' });
});

for (const count of [11, 160]) test(`${count} tabs keep a bounded grid strip and a searchable exact-target locator`, async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, count);
  await layoutAction(page, '3 rows × 3 columns');
  const cells = page.locator('[data-main-area] [data-view-group]'); await expect(cells).toHaveCount(9);
  const last = cells.last(), tabs = last.getByRole('tablist');
  const box = await tabs.boundingBox(), tab = await tabs.getByRole('tab').first().boundingBox();
  if (box === null || tab === null) throw new Error('The actual grid strip is missing');
  // One compact tab row plus its native scrollbar stays bounded independently of tab count.
  expect(box.height).toBeLessThanOrEqual(tab.height * 2);
  await expect(last.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await last.getByRole('button', { name: /^Find tab in / }).click();
  const chooser = page.getByRole('dialog', { name: /^Find tab in / });
  await chooser.getByRole('combobox', { name: 'Find an open tab', exact: true }).fill(`Grid agent ${count}`);
  const wanted = chooser.locator(`[role="option"][data-value="${tabId(count - 1)}"]`);
  await expect(wanted).toBeVisible(); await wanted.click();
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[count - 1]}$`));
  const selected = last.locator(`#tab-${tabId(count - 1)}`);
  await expect(selected).toBeFocused(); await expect(selected).toBeInViewport();
  await expect(last.getByRole('textbox', { name: 'Message to the agent' })).toBeVisible();
  await selected.press('Home'); await expect(last.locator(`#tab-${tabId(8)}`)).toBeFocused();
  await last.locator(`#tab-${tabId(8)}`).press('End'); await expect(selected).toBeFocused(); await expect(selected).toBeInViewport();
  await page.screenshot({ path: info.outputPath(`grid-${count}-tab-locator.png`), animations: 'disabled' });
});

test('two full Help panes keep their own section when focus and local navigation change', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 }); await start(page, 1);
  await page.goto('/help?part=settings');
  const firstTab = page.locator('[data-main-area] [role="tab"][aria-selected="true"]');
  const firstId = (await firstTab.getAttribute('id'))?.replace(/^tab-/, '');
  if (firstId === undefined) throw new Error('The first Help tab has no stable ID');
  const first = page.locator(`[data-view-instance="${firstId}"]`);
  await firstTab.click({ button: 'right' }); await page.getByRole('menuitem', { name: 'Open beside', exact: true }).click();
  const second = page.locator(`[data-view-instance]:not([data-view-instance="${firstId}"])`).filter({ has: page.getByRole('searchbox', { name: 'Search the help', exact: true }) });
  await expect(second).toBeVisible();
  await second.getByRole('tab', { name: 'Keys', exact: true }).click();
  await expect(page).toHaveURL(/part=keys/);
  await expect(first.getByRole('tab', { name: /^Settings/ })).toHaveAttribute('aria-selected', 'true');
  await first.getByRole('heading', { name: 'Help', exact: true }).click(); await expect(page).toHaveURL(/part=settings/);
  await expect(second.getByRole('tab', { name: 'Keys', exact: true })).toHaveAttribute('aria-selected', 'true');
  await page.screenshot({ path: info.outputPath('independent-help-sections.png'), animations: 'disabled' });
});

test('Given inspectors keep their own exact source when another thread opens or clears a record', async ({ page }, info) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const records = ['01960000-0000-7000-8020-000000000001', '01960000-0000-7000-8020-000000000002'];
  const reads: { thread: string; record: string | null }[] = [], errors: string[] = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.route('**/api/v1/nodes/*/model-inputs?*', async (route) => {
    const url = new URL(route.request().url()), thread = url.pathname.split('/').at(-2);
    if (thread === undefined) throw new Error('The actual model-input request must name its thread');
    reads.push({ thread, record: url.searchParams.get('id') });
    await route.fulfill({ json: { items: [], next_cursor: null } });
  });
  const instructions: ThreadInstructions = { context_revision: null, observed_at: null, segment_id: null, sources: [], standing: { characters: 0, limit: 20_000, pending_next_turn: false } };
  await page.route('**/api/v1/threads/*/instructions', (route) => route.fulfill({ json: instructions }));
  await start(page, 2);
  for (let index = 0; index < 2; index += 1) {
    await emit(ids[index]!, { items: [{ ...marker(`01960000-0000-7000-8021-${String(index + 1).padStart(12, '0')}`, { marker: 'orch_context', data: { model_input_id: records[index]!, purpose: 'input' } }), thread_id: ids[index]! }] });
  }
  await pane(page, 0).getByRole('button', { name: 'Context for the agent', exact: true }).click();
  await expect.poll(() => reads.some((entry) => entry.thread === ids[0] && entry.record === records[0])).toBe(true);
  await page.locator(`#tab-${tabId(1)}`).click();
  await pane(page, 1).getByRole('button', { name: 'Context for the agent', exact: true }).click();
  await expect.poll(() => reads.some((entry) => entry.thread === ids[1] && entry.record === records[1])).toBe(true);
  const first = page.locator('[data-view-instance]:has([data-testid="thread-given"])').filter({ hasText: 'Grid agent 1' });
  const second = page.locator('[data-view-instance]:has([data-testid="thread-given"])').filter({ hasText: 'Grid agent 2' });
  await expect(first).toBeVisible(); await expect(second).toBeVisible();
  await second.getByRole('button', { name: 'Show all', exact: true }).click();
  await expect.poll(() => reads.some((entry) => entry.thread === ids[1] && entry.record === null)).toBe(true);
  await expect(first.getByRole('button', { name: 'Show all', exact: true })).toBeVisible();
  expect(reads.some((entry) => entry.thread === ids[0] && entry.record === records[1] || entry.thread === ids[1] && entry.record === records[0])).toBe(false);
  await page.reload();
  await expect(first.getByRole('button', { name: 'Show all', exact: true })).toBeVisible();
  await expect(page.locator('[data-testid="thread-given"]')).toHaveCount(2);
  await expect(page).toHaveURL(new RegExp(`/thread/${ids[1]}$`));
  await page.screenshot({ path: info.outputPath('independent-given-records.png'), animations: 'disabled' });
  expect(errors).toEqual([]);
});
