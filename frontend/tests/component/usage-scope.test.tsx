/** Usage shares the authorized name locator: recents on focus and search across accessible scopes. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { ScopePicker } from '@/features/usage/ScopePicker';
import type { UsageScope } from '@/features/usage/search';
import type { NameHit } from '@/generated/api-types';
import { currentLocale, setLocale } from '@/i18n';
import { show } from './support';
import { i18n } from '@/i18n';

const reads = vi.hoisted(() => ({ names: vi.fn<(query: string) => Promise<NameHit[]>>() }));
vi.mock('@/api/search', () => ({ namesQuery: ({ q }: { q: string }) => ({ queryKey: ['scope-names', q], queryFn: () => reads.names(q) }) }));
const hit = (id: string, kind: 'org' | 'project', name: string, path: NameHit['path'] = []): NameHit => ({
  id, kind, name, path, address: null, avatar: null, archived: false, color: null, custom_color: null, owner: null,
  recent: true, tag: null, thread_attention: null, username: null,
});
const root = { id: 'acme', kind: 'org' as const, name: 'Acme', redacted: false };
const dept = { id: 'dept', kind: 'org' as const, name: 'Research', redacted: false };
beforeEach(() => {
  reads.names.mockReset();
  reads.names.mockImplementation((q) => Promise.resolve(q === '' ? [hit('beta', 'org', 'Beta'), hit('dept', 'org', 'Research', [root]), hit('project', 'project', 'Web', [root, dept])]
    : q === 'Other project' ? [hit('other', 'project', 'Other project', [{ ...root, id: 'beta', name: 'Beta' }])] : []));
});

it('updates a loading scope to the authorized place name without calling it Usage',()=>{
  const scope:UsageScope={kind:'node',id:'project'};
  const view=show(<ScopePicker scope={scope} name={null} onChange={()=>undefined} />);
  expect(screen.getByRole('combobox',{name:'Scope'})).toHaveValue(i18n.t('common.loading'));
  view.rerender(<ScopePicker scope={scope} name="Web" onChange={()=>undefined} />);
  expect(screen.getByRole('combobox',{name:'Scope'})).toHaveValue('Web');
  expect(reads.names).not.toHaveBeenCalled();
});

it('finds a project outside the recent org through the same field and retains its full path in the result', async () => {
  const changed = vi.fn(); show(<ScopePicker scope={{ kind: 'me' }} name={null} onChange={changed} />);
  const user = userEvent.setup(); const input = screen.getByRole('combobox', { name: 'Scope' });
  expect(reads.names).not.toHaveBeenCalled();
  await user.click(input); await user.type(input, 'Other project');
  const option = await screen.findByRole('option', { name: 'Other project' });
  expect(option).toHaveAccessibleDescription(/Beta/);
  await user.click(option);
  expect(changed).toHaveBeenCalledWith('node:other');
  expect(reads.names).toHaveBeenCalledWith('Other project');
  expect(screen.getAllByRole('combobox')).toHaveLength(1);
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('offers Me and authorized recent places without eagerly reading any tree (%s, %s)', async (locale, theme) => {
  const previous = currentLocale(); const oldTheme = document.documentElement.getAttribute('data-theme');
  await setLocale(locale); document.documentElement.setAttribute('data-theme', theme);
  const changed = vi.fn(); const view = show(<ScopePicker scope={{ kind: 'me' }} name={null} onChange={changed} />);
  try {
    const input = screen.getByRole('combobox', { name: locale === 'en' ? 'Scope' : '范围' });
    expect(input).toHaveValue(locale === 'en' ? 'Me' : '我'); expect(reads.names).not.toHaveBeenCalled();
    const user = userEvent.setup(); await user.click(input);
    const project = await screen.findByRole('option', { name: 'Web' });
    expect(project).toHaveAccessibleDescription(/Acme.*Research/);
    expect(screen.getByRole('option', { name: 'Beta' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Research' })).toBeInTheDocument();
    expect(reads.names).toHaveBeenCalledExactlyOnceWith('');
    await user.click(project); expect(changed).toHaveBeenCalledWith('node:project');
  } finally { view.unmount(); await setLocale(previous); if (oldTheme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', oldTheme); }
});
