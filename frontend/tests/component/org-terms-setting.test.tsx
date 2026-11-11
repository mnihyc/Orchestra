/** The registry text field previews native joining order, and member facts show only recorded acceptance. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { MemberTerms } from '@/features/auth/Terms';
import { SettingValue } from '@/features/settings/SettingValue';
import { localizedSetting } from '@/features/settings/settingText';
import type { Member, ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const def: SettingDef = { key: 'org.terms', title: 'Terms for people who join', description: 'Registry words', group: 'joining', class: 'policy', scopes: ['org'], merge: 'append', default: '', example: 'Keep private data here', schema: { type: 'string' }, text_key: null, keywords: [] };
const root = { id: 'root', kind: 'org', name: 'Acme', redacted: false } as const;
const here = { id: 'child', kind: 'org', name: 'Research', redacted: false } as const;
const setting: ResolvedSetting = {
  key: 'org.terms', value: '# Root\n\n# Local', own: { value: '# Local', revision: 3, locked: false },
  source: { scope: 'org', node: here }, locked_by: null, writable: true,
  parts: [
    { source: { scope: 'org', node: root }, value: '\n# Root\n', locked: false, ignored: false },
    { source: { scope: 'org', node: here }, value: '# Local', locked: false, ignored: false },
  ],
};
const member: Member = { terms_hash: null, terms_accepted_at: null, address: 'acme.ada', also: 0, domain: here, is_owner: false, last_seen_at: null, role: 'viewer', second_step: null, second_step_required: null, since: null, suspended: false, user: ADA };
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('keeps the own registry text separate and previews root then local in %s', async (locale) => {
  await setLocale(locale); const change = vi.fn();
  const localized = localizedSetting(def, i18n);
  expect(localized.title).toBe(locale === 'en' ? 'Terms for people who join' : '加入者须接受的条款');
  show(<SettingValue def={localized} setting={setting} target={{ kind: 'node', id: here.id }} shape={{ type: 'text', maxLength: null, pattern: null }} value="# Local" label={localized.title} onChange={change} />);
  const field = screen.getByRole('textbox', { name: localized.title });
  expect(field.tagName).toBe('TEXTAREA'); expect(field).toHaveValue('# Local');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('orgTerms.preview') }) }));
  const headings = screen.getAllByRole('heading').filter((heading) => ['Root', 'Local'].includes(heading.textContent));
  expect(headings.map((heading) => heading.textContent)).toEqual(['Root', 'Local']);
  expect(change).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('shows a known accepted date and leaves an old member record unknown in %s', async (locale) => {
  await setLocale(locale);
  const view = show(<MemberTerms member={member} />);
  expect(screen.queryByText(i18n.t('orgTerms.accepted'))).toBeNull();
  const accepted = { ...member, terms_hash: 'a'.repeat(64), terms_accepted_at: '2026-11-01T08:00:00Z' };
  view.rerender(<MemberTerms member={accepted} />);
  expect(screen.getByText(i18n.t('orgTerms.accepted'), { exact: false })).toBeVisible();
  expect(view.container.querySelector('time')).toHaveAttribute('dateTime', accepted.terms_accepted_at);
});
