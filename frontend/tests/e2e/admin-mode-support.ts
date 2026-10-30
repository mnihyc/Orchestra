/** The sole visible session switch is used explicitly by scenarios which need administrator reach. */
import { expect, type Page } from '@playwright/test';
import { PEOPLE, signIn } from './support.ts';

export async function asAdmin(page: Page, enabled: boolean): Promise<void> {
  const indicator = page.locator('[data-screen-titlebar]:visible').getByRole('switch', { name: /^As admin: (Off|On)$/ });
  await expect(indicator).toBeVisible();
  if (await indicator.getAttribute('data-as-admin') === String(enabled)) return;
  await indicator.click();
  await expect(indicator).toHaveAttribute('data-as-admin', String(enabled));
  await expect(indicator).toHaveAttribute('aria-checked', String(enabled));
}

interface BootstrapObservation { status: number; mode: unknown; readable: boolean }
type ObservedWindow = typeof window & { __managementBootstrap?: BootstrapObservation[] };

/** Management's existing scenarios opt into the session; ordinary signIn keeps the new default off. */
export async function signInForManagement(page: Page, person: { login: string; password: string } = PEOPLE.ada): Promise<void> {
  // Observe the same response inside the browser. Chromium's CDP body handle can
  // disappear even though the application has consumed and hydrated this answer.
  // Clone only this aggregate; return the original response and make no extra request.
  await page.addInitScript(() => {
    const observed = window as ObservedWindow;
    if (observed.__managementBootstrap !== undefined) return;
    const readings: BootstrapObservation[] = [];
    observed.__managementBootstrap = readings;
    const send = window.fetch.bind(window);
    window.fetch = (input, init) => {
      const sent = send(input, init);
      const address = input instanceof Request ? input.url : input instanceof URL ? input.href : input;
      if (new URL(address, location.href).pathname === '/api/v1/me/bootstrap') {
        void sent.then((reply) => {
          void reply.clone().json().then((body: unknown) => {
            const mode = typeof body === 'object' && body !== null && 'admin_mode' in body ? body.admin_mode : null;
            readings.push({ status: reply.status, mode, readable: true });
          }, () => readings.push({ status: reply.status, mode: null, readable: false }));
        }, () => readings.push({ status: 0, mode: null, readable: false }));
      }
      return sent;
    };
  });
  const state = page.waitForResponse((response) => response.request().method() === 'GET' && new URL(response.url()).pathname.endsWith('/me/bootstrap'));
  const [, reply] = await Promise.all([signIn(page, person), state]);
  expect(reply.status()).toBe(200);
  await expect.poll(() => page.evaluate(() => (window as ObservedWindow).__managementBootstrap?.length ?? 0)).toBe(1);
  const reading = await page.evaluate(() => (window as ObservedWindow).__managementBootstrap?.[0]);
  expect(reading).toMatchObject({ status: 200, readable: true });
  const mode = reading?.mode;
  expect(mode).toMatchObject({ available: expect.any(Boolean) });
  if (typeof mode === 'object' && mode !== null && 'available' in mode && mode.available === true) await asAdmin(page, true);
}
