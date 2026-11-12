/** Native terms refusals retire consent and keep the next attempt on its reviewed hash. */
import { expect, type Page, test } from '@playwright/test';
import type { InvitationPreview, Problem } from '../../src/generated/api-types';
import { ADA } from '../doubles/chat-rows';
import { scenario, signIn } from './support';

const TOKEN = 'terms-review-fixture';
const FIRST = 'a'.repeat(64), NEXT = 'b'.repeat(64), FINAL = 'c'.repeat(64);
async function language(page: Page, locale: 'en' | 'zh-CN') {
  if (locale === 'en') return;
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
  await page.getByRole('option', { name: '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`a changed native text requires a new choice and exact retry hash in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page); await language(page, locale);
    const preview: InvitationPreview = { kind: 'org', org: { name: 'Review team', slug: 'review-team' }, what: null, role: 'viewer', permissions: [], inviter: ADA,
      email_bound: false, expires_at: '2035-01-01T00:00:00Z', terms: { text: '# First review terms', hash: FIRST } };
    await page.route(`**/api/v1/invitations/lookup/${TOKEN}`, (route) => route.fulfill({ json: preview }));
    const sent: unknown[] = [];
    await page.route('**/api/v1/invitations/accept', async (route) => {
      const body: unknown = route.request().postDataJSON();
      sent.push(body);
      const terms = sent.length === 1 ? { text: '# Second review terms', hash: NEXT } : { text: '# Final review terms', hash: FINAL };
      const refusal: Problem = { type: 'orch:error:invitation.terms_not_accepted', code: 'invitation.terms_not_accepted', status: 409,
        title: 'Terms changed', detail: '', request_id: 'terms-review', terms };
      await route.fulfill({ status: 409, json: refusal });
    });
    await page.goto(`/invite/${TOKEN}`);
    const checkbox = page.getByRole('checkbox');
    const accept = page.getByRole('button', { name: locale === 'en' ? 'Accept invitation' : '接受邀请', exact: true });
    await expect(page.getByRole('heading', { name: 'First review terms' })).toBeVisible();
    await expect(accept).toBeDisabled();
    await checkbox.check(); await expect(accept).toBeEnabled(); await accept.click();
    await expect(page.getByRole('heading', { name: 'Second review terms' })).toBeVisible();
    await expect(checkbox).not.toBeChecked(); await expect(accept).toBeDisabled();
    expect(sent).toEqual([{ token: TOKEN, accepted_terms_hash: FIRST }]);
    await checkbox.check(); await accept.click();
    await expect(page.getByRole('heading', { name: 'Final review terms' })).toBeVisible();
    await expect(checkbox).not.toBeChecked(); await expect(accept).toBeDisabled();
    expect(sent).toEqual([{ token: TOKEN, accepted_terms_hash: FIRST }, { token: TOKEN, accepted_terms_hash: NEXT }]);
  });
}
