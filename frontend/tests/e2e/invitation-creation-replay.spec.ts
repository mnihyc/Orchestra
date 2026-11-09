/** Each mounted invitation door keeps its retry key and exposes no repeated secret. */
import { expect, test } from '@playwright/test';
import en from '../../src/i18n/en.json' with { type: 'json' };
import zh from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { signInForManagement } from './admin-mode-support';
import { ACME, scenario } from './support';

const ID = '00000000-0000-4000-8000-000000000111';
for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440])
for (const theme of ['light', 'dark'] as const) for (const [kind, form] of [['org', 'link'], ['org', 'email'], ['system', 'link'], ['system', 'email'], ['outsider', 'email']] as const) {
  test(`invitation resend and deliberate replacement in ${kind}/${form}/${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario({ meta: { mail: false } });
    await page.addInitScript(({ theme }) => {
      localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' }));
    }, { theme });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signInForManagement(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? en : zh;
    const attempts: { key: string | undefined; body: string | null }[] = [];
    let revokes = 0;
    await page.route('**/api/v1/invitations', async (route) => {
      if (route.request().method() !== 'POST') { await route.fallback(); return; }
      attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
      if (attempts.length === 1) { await route.abort('connectionfailed'); return; }
      await route.fulfill({ json: attempts.length === 2
        ? { projection: 'receipt', invitation_id: ID, token: null, url: null, sent: false, already_created: true }
        : { projection: 'receipt', invitation_id: '00000000-0000-4000-8000-000000000112', token: 'new-first-only', url: 'https://example.test/invite/new-first-only', sent: false } });
    });
    await page.route(`**/api/v1/invitations/${ID}`, async (route) => {
      if (route.request().method() !== 'DELETE') { await route.fallback(); return; }
      revokes += 1;
      if (revokes === 1) await route.fulfill({ status: 403, json: { type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Refused', detail: '', request_id: 'invitation-revoke' } });
      else await route.fulfill({ status: 204, body: '' });
    });
    if (kind === 'org') {
      await page.goto('/org/acme/settings/members');
      await page.getByRole('button', { name: words.places.members.invite, exact: true }).click();
    } else if (kind === 'system') {
      await page.goto('/admin/users');
      await page.getByRole('button', { name: words.admin.users.invite.open, exact: true }).click();
    } else {
      await page.goto(`/topic/${ACME.fixLogin}`);
      await page.getByRole('button', { name: words.places.menu.of.replace('{{name}}', 'Fix login'), exact: true }).first().click();
      await page.getByRole('menuitem', { name: locale === 'en' ? 'Share…' : '共享…', exact: true }).click();
      const access = page.getByRole('dialog');
      const options = access.getByRole('button', { name: words.access.roster.options, exact: true });
      if (await options.getAttribute('aria-expanded') !== 'true') await options.click();
      await access.getByRole('combobox', { name: words.access.reach.label }).click();
      await page.getByRole('option', { name: words.choiceCatalog.reach.alone.name, exact: true }).click();
      await access.getByRole('combobox', { name: words.access.add.label, exact: true }).fill('new@example.test');
      await page.getByRole('option', { name: words.access.candidates.inviteTo.replace('{{email}}', 'new@example.test'), exact: true }).click();
    }
    const dialog = page.getByRole('dialog').last();
    const submitName = kind === 'outsider' ? words.access.outsiders.inviteSend : words.places.invite.submit;
    if (kind !== 'outsider') {
      await dialog.getByRole('radio', { name: words.choiceCatalog.invitationForm[form].name, exact: true }).click();
      if (form === 'email') await dialog.getByRole('textbox', { name: words.places.invite.email, exact: true }).fill('new@example.test');
    }
    const submit = dialog.getByRole('button', { name: submitName, exact: true });
    await expect(submit).toBeEnabled();
    await info.attach('invitation-before', { body: await page.screenshot(), contentType: 'image/png' });
    await submit.click();
    await expect(dialog.getByRole('alert')).toBeVisible();
    await submit.focus(); await page.keyboard.press('Enter');
    await expect(dialog.getByText(words.places.invite.alreadyCreated, { exact: true })).toBeVisible();
    expect(attempts).toHaveLength(2);
    expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    await expect(dialog.getByRole('textbox', { name: kind === 'outsider' ? words.access.outsiders.link : words.places.invite.link })).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: kind === 'outsider' ? words.access.outsiders.copyLink : words.places.invite.copy })).toHaveCount(0);
    const revoke = dialog.getByRole('button', { name: words.places.invite.revokeAndAnother, exact: true });
    await revoke.focus(); await page.keyboard.press('Enter');
    const confirmation = page.getByRole('alertdialog', { name: words.places.members.revokeTitle, exact: true });
    const confirm = confirmation.getByRole('button', { name: words.places.members.revokeConfirm, exact: true });
    await confirm.click();
    await expect(confirmation.getByRole('alert')).toBeVisible();
    expect(revokes).toBe(1); expect(attempts).toHaveLength(2);
    await expect(page.getByRole('button', { name: submitName, exact: true })).toHaveCount(0);
    await confirm.click();
    await expect(submit).toBeVisible();
    if (kind !== 'outsider' && form === 'email') await dialog.getByRole('textbox', { name: words.places.invite.email, exact: true }).fill('new@example.test');
    await submit.click();
    await expect.poll(() => attempts.length).toBe(3);
    expect(attempts[2]?.body).toBe(attempts[0]?.body);
    expect(attempts[2]?.key).not.toBe(attempts[0]?.key);
    await expect(dialog.getByRole('textbox', { name: kind === 'outsider' ? words.access.outsiders.link : words.places.invite.link })).toHaveValue('https://example.test/invite/new-first-only');
    expect(revokes).toBe(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('invitation-after', { body: await page.screenshot(), contentType: 'image/png' });
  });
}
