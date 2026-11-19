/** The screens a person meets before they are signed in, asked in both themes whether anything on them is a serious barrier: on a phone, in Chinese, with a refusal said at its field, and every link that does not lead anywhere any more; and worked by keyboard alone. */
import { expect, type Page, test } from '@playwright/test';

import { expectAccessibleInBothThemes, PEOPLE, scenario } from '../e2e/support.ts';

test.beforeEach(async () => {
  await scenario();
});

const PHONE = { width: 390, height: 844 };

/** The language picked on the page itself, as a person without an account does it. */
async function inChinese(page: Page): Promise<void> {
  await page.getByRole('radio', { name: '简体中文' }).click();
  await expect(page.locator('html')).toHaveAttribute('lang', 'zh-CN');
}

test('sign-in, recovery and registration on a phone', async ({ page }) => {
  await page.setViewportSize(PHONE);
  await page.goto('/login');
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByRole('link', { name: 'Forgot your password?' }).click();
  await expect(page).toHaveURL(/\/recover$/);
  await expectAccessibleInBothThemes(page);

  await page.goto('/register');
  await expect(page.getByRole('heading').first()).toBeVisible();
  await expectAccessibleInBothThemes(page);
});

test('sign-in in Chinese, with a wrong password said at its field', async ({ page }) => {
  await page.goto('/login');
  await inChinese(page);
  await expect(page.getByRole('heading', { name: '登录' })).toBeVisible();
  await expectAccessibleInBothThemes(page);

  await page.getByLabel('用户名或邮箱地址').fill(PEOPLE.ada.login);
  await page.getByLabel('密码', { exact: true }).fill('not her password');
  await page.getByRole('button', { name: '登录', exact: true }).click();
  await expect(page.getByLabel('密码', { exact: true })).toBeFocused();
  // The refusal is tied to its field, so that a screen reader reads it with the field.
  await expect(page.getByLabel('密码', { exact: true })).toHaveAttribute('aria-invalid', 'true');
  await expectAccessibleInBothThemes(page);
});

test('links that lead nowhere any more: a reset, a confirmation and an invitation, in both languages', async ({ page }) => {
  for (const path of ['/reset/not-a-token', '/verify-email/not-a-token', '/invite/spent-invitation']) {
    await page.goto(path);
    await expect(page.getByRole('heading').first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
  await page.goto('/login');
  await inChinese(page);
  for (const path of ['/reset/not-a-token', '/verify-email/not-a-token', '/invite/spent-invitation']) {
    await page.goto(path);
    await expect(page.getByRole('heading').first()).toBeVisible();
    await expectAccessibleInBothThemes(page);
  }
});

test('sign-in is worked by keyboard alone, the language and the theme included', async ({ page }) => {
  await page.goto('/login');
  await expect(page.getByLabel('Username or email address')).toBeFocused();
  await page.keyboard.type(PEOPLE.ada.login);
  // Typing the name goes straight to the password; recovery follows the field in the DOM.
  await page.keyboard.press('Tab');
  await expect(page.getByLabel('Password', { exact: true })).toBeFocused();
  await page.keyboard.type(PEOPLE.ada.password);
  // Every stop after the password is something one can see and use: the visibility of the password, the act, the ways out.
  const stops: string[] = [];
  for (let press = 0; press < 8; press += 1) {
    await page.keyboard.press('Tab');
    stops.push(await page.evaluate(() => {
      const element = document.activeElement as HTMLElement | null;
      if (element === null || element === document.body) return '';
      const box = element.getBoundingClientRect();
      const style = getComputedStyle(element);
      const ring = (style.outlineStyle !== 'none' && parseFloat(style.outlineWidth) > 0) || style.boxShadow !== 'none';
      return `${box.width > 0 && box.height > 0 ? 'seen' : 'unseen'}:${ring ? 'ring' : 'no-ring'}:${element.getAttribute('aria-label') ?? element.textContent.trim()}`;
    }));
  }
  expect(stops.filter((stop) => stop !== '' && !stop.startsWith('seen:ring')), stops.join(' | ')).toEqual([]);
  await page.getByRole('button', { name: 'Sign in' }).focus();
  await page.keyboard.press('Enter');
  await expect(page).toHaveURL(/\/org\//);
});
