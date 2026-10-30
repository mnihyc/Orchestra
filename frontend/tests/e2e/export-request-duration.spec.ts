/** Export access uses the same duration request at phone and desktop widths in both themes. */
import { expect, test } from '@playwright/test';
import type { AccessRequestCreateBody, AccessRequestCreateReply } from '../../src/generated/api-types';
import { ACME, PEOPLE, bodySent, replySent, scenario, signIn } from './support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`an export request carries a duration at ${width}px in ${theme}`, async ({ page, request }) => {
    await scenario();
    expect((await request.post(`${DOUBLE}/__double/export/bar`, { data: { username: 'dora', permission: 'thread.export', object: ACME.repro } })).ok()).toBe(true);
    await page.setViewportSize({ width, height: 1000 });
    await signIn(page, PEOPLE.dora);
    const login = await request.post(`${DOUBLE}/api/v1/auth/login`, { data: { login: PEOPLE.dora.login, password: PEOPLE.dora.password } });
    expect(login.ok()).toBe(true);
    const { access_token: token } = await login.json() as { access_token: string };
    expect((await request.patch(`${DOUBLE}/api/v1/me`, { headers: { Authorization: `Bearer ${token}` }, data: { locale: 'en', theme } })).ok()).toBe(true);
    let requests = 0;
    page.on('request', (entry) => { if (new URL(entry.url()).pathname.startsWith('/api/')) requests += 1; });
    await page.goto(`/thread/${ACME.repro}`);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.locator('[data-thread-header]').getByRole('button', { name: /^Actions for (Main thread|Repro)/ }).click();
    await page.getByRole('menuitem', { name: 'Request access' }).click();
    const dialog = page.getByRole('dialog', { name: 'Request access to export Repro' });
    const duration = dialog.getByRole('combobox', { name: 'How long do you need access?' });
    await duration.click();
    await page.getByRole('option', { name: '1 week', exact: true }).click();
    const bounds = await dialog.boundingBox();
    expect(bounds).not.toBeNull();
    if (bounds === null) throw new Error('The request has no bounds');
    expect(bounds.x).toBeGreaterThanOrEqual(0);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(width);
    const path = test.info().outputPath(`export-request-${width}-${theme}.png`);
    await page.screenshot({ path, animations: 'disabled' });
    await test.info().attach('Export request', { path, contentType: 'image/png' });
    await test.info().attach('API requests', { body: String(requests), contentType: 'text/plain' });
    const before = Date.now();
    const sent = page.waitForResponse((entry) => entry.request().method() === 'POST' && new URL(entry.url()).pathname.endsWith('/access-requests'));
    await dialog.getByRole('button', { name: 'Send request' }).click();
    const response = await sent;
    expect(response.ok()).toBe(true);
    const body = await bodySent(response.request()) as AccessRequestCreateBody;
    expect(body.items).toEqual([{ object: { kind: 'thread', id: ACME.repro }, permissions: ['thread.export'] }]);
    const reply = await replySent(response) as AccessRequestCreateReply;
    expect(reply.approvals).toHaveLength(1);
    const approval = reply.approvals[0];
    if (approval?.kind !== 'access') throw new Error('The server did not return an access request');
    expect(approval.object).toMatchObject({ kind: 'thread', id: ACME.repro });
    expect(approval.request.permissions).toEqual(['thread.export']);
    expect(approval.request.expires_at).toBe(body.expires_at);
    const end = Date.parse(approval.request.expires_at ?? '');
    expect(end).toBeGreaterThanOrEqual(before + 7 * 24 * 60 * 60 * 1000);
    expect(end).toBeLessThanOrEqual(Date.now() + 7 * 24 * 60 * 60 * 1000);
  });
}
