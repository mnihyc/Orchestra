/** Reveal actions identify their own field; changing one never reveals either neighbouring password. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { PasswordInput } from '@/ui/Input';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });
it.each(['en', 'zh-CN'] as const)('names each reveal control in a three-password form in %s', async (locale) => {
  await setLocale(locale);
  const fields = locale === 'en' ? ['Current password', 'New password', 'New password again'] : ['当前密码', '新密码', '再次输入新密码'];
  show(<form>{fields.map((field) => <PasswordInput key={field} label={field} defaultValue="kept-secret" />)}</form>);
  for (const field of fields) expect(screen.getByRole('button', { name: i18n.t('ui.password.showField', { field }) })).toBeVisible();
  const [current, next, repeated] = fields;
  if (current === undefined || next === undefined || repeated === undefined) throw new Error('The three password fields are missing');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('ui.password.showField', { field: next }) }));
  expect(screen.getByLabelText(next, { exact: true })).toHaveAttribute('type', 'text');
  expect(screen.getByRole('button', { name: i18n.t('ui.password.hideField', { field: next }) })).toHaveAttribute('aria-pressed', 'true');
  expect(screen.getByLabelText(current, { exact: true })).toHaveAttribute('type', 'password');
  expect(screen.getByLabelText(repeated, { exact: true })).toHaveAttribute('type', 'password');
});

it.each(['en', 'zh-CN'] as const)('keeps the familiar single Password reveal name in %s', async (locale) => {
  await setLocale(locale);
  show(<PasswordInput label={i18n.t('auth.fields.password')} />);
  expect(screen.getByRole('button', { name: i18n.t('ui.password.show') })).toBeVisible();
});
