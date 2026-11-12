/** The combination matrix of what waits for a person: a boundary request of each risk and a person's access request,
 * read and decided in each language, theme and size of screen.
 *
 * Expected: every request is a card in the Approvals channel with its decisions named for what they decide, in the
 * person's language, with one card for each request; a capability is decided from
 * the card on a phone as on a desktop; and the page has no
 * serious or critical accessibility barrier in either theme. Replay one case with
 * `ORCH_MATRIX_CASE=<id>` (see `matrix-kit.ts`). */
import { expect, type Page, test } from '@playwright/test';

import { approvalsMade, askAccess, askCapability } from './inbox-support.ts';
import { cases, title } from './matrix-kit.ts';
import { ACME, expectAccessible, scenario, signIn } from './support.ts';

const SIZES = { desktop: { width: 1280, height: 800 }, phone: { width: 390, height: 844 } } as const;

const MATRIX = cases(
  'approvals',
  [
    { name: 'request', values: ['capability_low', 'capability_medium', 'capability_high', 'capability_critical', 'access'] },
    { name: 'locale', values: ['en', 'zh-CN'] },
    { name: 'theme', values: ['light', 'dark'] },
    { name: 'screen', values: ['desktop', 'phone'] },
  ],
  () => true,
  { pairSeed: 23, interacting: [['request', 'screen']], randomSeed: 20261015, random: 4 },
);

/** Language changes use the Interface control, then come back to the screen (as `craft.spec.ts` does). */
async function language(page: Page, locale: 'en' | 'zh-CN'): Promise<void> {
  const back = page.url();
  await page.goto('/settings/interface');
  await page.getByRole('combobox', { name: /^(?:Language|语言)$/ }).click();
  await page.getByRole('option', { name: locale === 'en' ? 'English' : '简体中文', exact: true }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
  await page.goto(back);
}

test.beforeEach(async () => {
  await scenario();
});

for (const each of MATRIX) {
  test(`matrix ${title(each)}`, async ({ page }) => {
    const request = each.values.request ?? 'access';
    const locale = each.values.locale as 'en' | 'zh-CN';
    const theme = each.values.theme as 'light' | 'dark';
    const name = `Matrix ${each.id.split('/').slice(1).join(' ')}`;
    const approval = request === 'access'
      ? await askAccess({ requester: 'vera', title: name, permissions: ['thread.view'], object: { kind: 'thread', id: ACME.mainThread, name: 'Main thread' } })
      : await askCapability({ requester: 'grace', title: name, risk: request.replace('capability_', '') as 'low' | 'medium' | 'high' | 'critical' });
    await page.setViewportSize(SIZES[each.values.screen as keyof typeof SIZES]);
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    await page.goto('/approvals');
    await language(page, locale);

    // Capability cards intentionally name their native subject; the matrix title is fixture metadata.
    const subject = request === 'access' ? name : 'GPU';
    const approve = locale === 'en' ? `Approve ${subject}` : `批准：${subject}`;
    const deny = locale === 'en' ? `Deny ${subject}` : `拒绝：${subject}`;
    const card = page.locator(`[data-approval="${approval.id}"]`);
    await expect(card).toHaveCount(1);
    await card.scrollIntoViewIfNeeded();
    await expect(card).toBeVisible();
    await expect(card).toHaveAccessibleName(subject);
    // Access cards keep the full Approve/Deny form behind their explicit decision-details action.
    if (request === 'access') await card.getByRole('button', { name: locale === 'en' ? 'Other…' : '其他…', exact: true }).click();
    await expect(card.getByRole('button', { name: approve, exact: true })).toBeVisible();
    await expect(card.getByRole('button', { name: deny, exact: true })).toBeVisible();
    await expectAccessible(page, theme);

    if (request !== 'access') {
      await card.getByRole('button', { name: approve, exact: true }).click();
      await expect(card.getByRole('button', { name: approve, exact: true })).toHaveCount(0);
      await expect.poll(async () => (await approvalsMade()).find((row) => row.id === approval.id)?.state).toBe('approved');
    }
  });
}
