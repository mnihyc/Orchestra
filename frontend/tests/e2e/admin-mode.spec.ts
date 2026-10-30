/** Session reach is explicit, keeps its title-bar reminder, and never edits a permission page to open content. */
import { expect, test } from '@playwright/test';
import { asAdmin } from './admin-mode-support.ts';
import { secondReader } from './chat-support.ts';
import { ACME, PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function privateDocument(): Promise<string> {
  const response = await fetch(`${DOUBLE}/__double/documents/make`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ user: 'dora', node: ACME.fixLogin, title: 'A private canary', private: true, count: 1 }) });
  if (!response.ok) throw new Error(`The private document fixture failed: ${response.status}`);
  const answer: unknown = await response.json();
  if (typeof answer !== 'object' || answer === null || !('id' in answer) || typeof answer.id !== 'string') throw new Error('The fixture returned no document identifier');
  return answer.id;
}
test.beforeEach(async () => { await scenario(); });

test('a system administrator reaches Management from the account menu with mode off and enables it there with one act', async ({ page }) => {
  // Chromium's network trace omits this streamed Request body. Observe a clone
  // before transport; the original fetch arguments and response are untouched.
  await page.addInitScript(() => {
    const observed = window as typeof window & { __orchAdminWrites: unknown[] };
    observed.__orchAdminWrites = [];
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      if (input instanceof Request && input.method === 'PUT' && new URL(input.url).pathname.endsWith('/me/admin-mode')) {
        void input.clone().json().then((body: unknown) => { observed.__orchAdminWrites.push(body); }, () => { observed.__orchAdminWrites.push('Unreadable JSON'); });
      }
      return send(input, init);
    };
  });
  await signIn(page);
  const control = page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: Off', exact: true });
  await expect(control).not.toBeChecked();
  await page.getByRole('button', { name: 'Your account', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Management', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Management', level: 1 })).toBeVisible();
  const writes: string[] = [];
  page.on('request', (request) => { if (request.method() === 'PUT' && new URL(request.url()).pathname.endsWith('/me/admin-mode')) writes.push(request.method()); });
  await expect(page.getByRole('switch', { name: /^As admin:/ })).toHaveCount(1);
  await page.getByRole('button', { name: 'Turn on As admin', exact: true }).click();
  await expect(page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: On', exact: true })).toBeChecked();
  await expect(page.getByRole('heading', { name: 'Overview', level: 1 })).toBeVisible();
  expect(writes).toEqual(['PUT']);
  expect(await page.evaluate(() => (window as typeof window & { __orchAdminWrites: unknown[] }).__orchAdminWrites)).toEqual([{ as_admin: true }]);
});

test('the single direct switch fits its icon and state below 600px and keeps its full accessible name', async ({ page }) => {
  await signIn(page);
  const control = page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: /^As admin: (Off|On)$/ });
  await page.setViewportSize({ width: 400, height: 860 });
  await expect(control).toHaveAccessibleName('As admin: Off');
  const row = control.locator('xpath=ancestor::*[@data-choice-layout]');
  await expect(row.getByText('Off', { exact: true })).toBeVisible();
  await expect(row.getByText('As admin: Off', { exact: true })).toBeHidden();
  const size = await control.evaluate((element) => {
    const thumb = element.querySelector('[data-choice-thumb]');
    const label = element.closest('[data-choice-layout]')?.querySelector('label');
    if (thumb === null || label === null || label === undefined) throw new Error('The shared switch lost its label or thumb');
    const track = element.getBoundingClientRect(), knob = thumb.getBoundingClientRect(), words = label.getBoundingClientRect();
    return { track: { x: track.x, y: track.y, width: track.width, height: track.height },
      knob: { x: knob.x, right: knob.right, width: knob.width }, labelRight: words.right,
      background: getComputedStyle(element).backgroundColor, thumb: getComputedStyle(thumb).backgroundColor };
  });
  expect(size.track.width).toBe(32); expect(size.track.height).toBe(20);
  expect(size.knob.width).toBe(16); expect(size.knob.x).toBeGreaterThanOrEqual(size.track.x);
  expect(size.knob.right).toBeLessThanOrEqual(size.track.x + size.track.width);
  expect(size.labelRight).toBeLessThan(size.track.x);
  expect(size.track.x + size.track.width).toBeLessThanOrEqual(400);
  expect(size.thumb).toBe('rgb(255, 255, 255)');
  await control.focus();
  await page.keyboard.press('Space');
  await expect(control).toBeChecked();
  await expect(control).toHaveAccessibleName('As admin: On');
  await expect(page.getByRole('dialog', { name: 'As admin', exact: true })).toHaveCount(0);
  await expect(page.getByRole('switch', { name: /^As admin:/ })).toHaveCount(1);
  await page.setViewportSize({ width: 600, height: 860 });
  await expect(row.getByText('As admin: On', { exact: true })).toBeVisible();
  await expect(row.getByText('On', { exact: true })).toBeHidden();
});

test('the same person reaches private content only with their current session switch on, without a self-grant', async ({ page, browser, baseURL }) => {
  const document = await privateDocument();
  const path = `/doc/${document}`;
  const pageWrites: string[] = [];
  page.on('request', (request) => {
    if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method()) && /\/(?:access|permissions|roles)(?:\/|$)/.test(new URL(request.url()).pathname)) pageWrites.push(request.url());
  });
  await signIn(page);
  const indicator = page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: /^As admin: (Off|On)$/ });
  await expect(indicator).toHaveAttribute('data-as-admin', 'false');
  await expect(indicator).toHaveAccessibleName('As admin: Off');
  await expect(indicator).not.toBeChecked();
  await page.goto(path);
  await expect(page.locator('[data-main-area]').getByRole('alert')).toHaveText('This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager.');
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Open.*admin/ })).toHaveCount(0);
  await asAdmin(page, true);
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toBeVisible();
  await expect(page.getByRole('img', { name: 'Private · You see this as a system admin', exact: true })).toBeVisible();
  await page.reload();
  await expect(indicator).toHaveAttribute('data-as-admin', 'true');
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toBeVisible();

  const other = await secondReader(browser, baseURL, path, PEOPLE.ada);
  await expect(other.page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: Off', exact: true })).not.toBeChecked();
  await expect(other.page.locator('[data-main-area]').getByRole('alert')).toHaveText('This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager.');
  await expect(other.page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toHaveCount(0);
  // The document arrangement already retains Comments when this target is unavailable.
  // New contextual offers are correctly absent from the view chooser after access refusal.
  const comments = other.page.locator('[data-section-view="comments"]');
  await expect(comments).toBeVisible();
  await expect(comments.getByRole('alert')).toHaveText('This does not exist, was deleted, or is not open to you. Return to a place you can open or ask its manager.');
  await expect(comments.getByText('Loading…', { exact: true })).toHaveCount(0);
  const unavailableImage = test.info().outputPath('private-document-unavailable-comments.png');
  await other.page.screenshot({ path: unavailableImage, fullPage: true, animations: 'disabled' });
  await test.info().attach('private-document-unavailable-comments', { path: unavailableImage, contentType: 'image/png' });
  await other.context.close();
  await asAdmin(page, false);
  await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}(?:\\?|$)`));
  await expect(page.locator('[data-main-area] [role="alert"]:visible')).toHaveCount(0);
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toHaveCount(0);
  expect(pageWrites).toEqual([]);
});

test('Management remains in a system administrator’s account menu with mode off, and the old switches are absent', async ({ page }) => {
  await signIn(page);
  await page.getByRole('button', { name: 'Your account' }).click();
  await expect(page.getByRole('menuitem', { name: 'Management', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await asAdmin(page, true);
  await page.getByRole('button', { name: 'Your account' }).click();
  await expect(page.getByRole('menuitem', { name: 'Management', exact: true })).toBeVisible();
  await page.keyboard.press('Escape');
  await page.goto('/org/acme/messages');
  await expect(page.getByRole('switch', { name: /Show all chats/ })).toHaveCount(0);
  await expect(page.getByRole('switch', { name: 'Everything I can see', exact: true })).toBeVisible();
  await asAdmin(page, false);
  await page.goto('/admin/overview');
  await expect(page.getByRole('heading', { level: 1, name: 'Management', exact: true })).toBeVisible();
  await expect(page.getByRole('navigation', { name: 'Management sections' })).toHaveCount(0);
  await expect(page.getByRole('switch', { name: /^As admin: (Off|On)$/ })).toHaveCount(1);
  await asAdmin(page, true);
  await expect(page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: 'As admin: On', exact: true })).toBeChecked();
  await expect(page.getByRole('navigation', { name: 'Management sections' })).toBeVisible();
});

test('a private opening is shown in the audit with the As admin fact', async ({ page }) => {
  const document = await privateDocument();
  await signIn(page);
  await asAdmin(page, true);
  await page.goto(`/doc/${document}`);
  await expect(page.getByText('Paragraph 1: the quick brown fox jumps over the lazy dog.')).toBeVisible();
  await page.goto('/admin/audit');
  await expect(page.locator(`[data-trace-object="${document}"]`).filter({ hasText: 'As admin' }).first()).toBeVisible();
});

test('the finder’s ordinary focus is keyboard-operated without activating its selected result', async ({ page }) => {
  await signIn(page);
  await page.goto('/org/acme');
  // A hard navigation must mount the real shell before a person can use its shortcut.
  await page.getByRole('button', { name: 'Find anything', exact: true }).focus();
  await page.keyboard.press('Control+k');
  const finder = page.getByRole('dialog', { name: 'Find', exact: true });
  await finder.getByRole('combobox', { name: 'Search terms', exact: true }).fill('fix');
  const focus = finder.getByRole('switch');
  await focus.focus();
  const narrowed = page.waitForResponse((response) => {
    const url = new URL(response.url());
    return url.pathname.endsWith('/search/names') && url.searchParams.get('participating') === 'true';
  });
  await page.keyboard.press('Space');
  await narrowed;
  await expect(focus).toBeChecked();
  await expect(finder.getByRole('option', { name: /Fix login/ }).first()).toBeVisible();
  await focus.focus();
  await page.keyboard.press('Alt+Enter');
  await expect(finder).toBeVisible();
  await expect(page).toHaveURL(/\/org\/acme$/);
});
