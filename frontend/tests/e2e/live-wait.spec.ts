/** A wait that only the live connection could end ends without it: the socket is refused, as by a front that does not pass the upgrade, and the dialog still sees the machine come. */
import { expect, type Page, test } from '@playwright/test';

import words from '../../src/i18n/en.json' with { type: 'json' };
import { PEOPLE, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

async function resourceControl(path: string, body: unknown = {}): Promise<void> {
  const response = await fetch(`${DOUBLE}/__double/resources${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fake API refused ${path}: ${response.status}`);
}

/** A front that does not pass the upgrade: the socket never opens, it errors and closes (1006) at once, and again at every try. */
async function refuseSocket(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const Native = window.WebSocket;
    const Refused = function (this: unknown, url: string | URL, protocol?: string | string[]) {
      if (!String(url).includes('/api/v1/ws')) return new Native(url, protocol);
      const socket: Record<string, unknown> = { readyState: 0, url: String(url), protocol: '', onopen: null, onclose: null, onmessage: null, onerror: null, send() { /* nothing */ }, close() { /* nothing */ }, addEventListener() { /* nothing */ }, removeEventListener() { /* nothing */ } };
      setTimeout(() => { socket.readyState = 3; (socket.onerror as ((event: unknown) => void) | null)?.({}); (socket.onclose as ((event: unknown) => void) | null)?.({ code: 1006, reason: '', wasClean: false }); }, 30);
      return socket;
    } as unknown as typeof WebSocket;
    Object.assign(Refused, { CONNECTING: 0, OPEN: 1, CLOSING: 2, CLOSED: 3, prototype: Native.prototype });
    window.WebSocket = Refused;
  });
}

test.beforeEach(async () => { await scenario(); });

for (const width of [1440, 400]) {
  test(`with the socket refused, the Add machine dialog sees the machine come and opens it (${width})`, async ({ page }) => {
    await refuseSocket(page);
    await page.setViewportSize({ width, height: 900 });
    await signIn(page, PEOPLE.ada);
    await page.goto('/resources/machines');
    await page.getByRole('button', { name: 'Add machine' }).first().click();
    const dialog = page.getByRole('dialog', { name: 'Add machine' });
    await dialog.getByRole('button', { name: 'Show command' }).click();
    await expect(dialog.getByText(words.machines.add.waiting)).toBeVisible();
    const token = /--token '([^']+)'/.exec(await dialog.locator('pre code').first().innerText())?.[1] ?? /-Token '([^']+)'/.exec(await dialog.locator('pre code').first().innerText())?.[1];
    if (token === undefined) throw new Error('the command has no enrollment token');
    await resourceControl('/enroll', { name: 'build-vm', token });
    // No event can come. The dialog reads again, and finds it.
    await expect(dialog).toHaveCount(0, { timeout: 15_000 });
    await expect(page.getByRole('heading', { name: 'build-vm', level: 1 })).toBeVisible();
  });
}

test('with the socket refused, the shell says so after ten seconds and names the check', async ({ page }) => {
  await refuseSocket(page);
  await page.setViewportSize({ width: 1440, height: 900 });
  await signIn(page, PEOPLE.ada);
  await page.goto('/resources/machines');
  await expect(page.getByText(words.workbench.status.connecting, { exact: true })).toBeVisible();
  const lost = page.getByText(/^No live updates · /);
  await expect(lost).toBeVisible({ timeout: 20_000 });
  await expect(page.getByText(words.workbench.status.connecting, { exact: true })).toHaveCount(0);
  await lost.hover();
  await expect(page.getByRole('tooltip').getByText(/WebSocket upgrades for \/api\/v1\/ws/)).toBeVisible();
});
