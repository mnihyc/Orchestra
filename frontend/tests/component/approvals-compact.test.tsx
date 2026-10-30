/** R1b S61: the real Approvals page has one heading, no subtitle and the same native list controls. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as ApprovalApi from '@/api/approvals';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { Approvals, type ApprovalsTab } from '@/features/approvals/Approvals';
import type { ApprovalKind, ApprovalPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { choiceOption } from '@/i18n/choices';
import { useAppearance } from '@/lib/appearance';
import { show } from './support';
import { retiredCopy } from './retired-copy';

const read = vi.hoisted(() => vi.fn<(_filter: ApprovalApi.ApprovalsFilter) => Promise<ApprovalPage>>());
vi.mock('@/api/approvals', async (original) => {
  const api = await original<typeof ApprovalApi>();
  return { ...api, approvalsQuery: (filter: ApprovalApi.ApprovalsFilter) => ({ ...api.approvalsQuery(filter), queryFn: () => read(filter) }) };
});
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useUserFeed: () => undefined }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearanceBefore = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');

function Page() {
  const [tab, setTab] = useState<ApprovalsTab>('waiting');
  const [all, setAll] = useState(false);
  const [kind, setKind] = useState<ApprovalKind | null>(null);
  return <Approvals tab={tab} onTab={setTab} all={all} onAll={setAll} kind={kind} onKind={setKind} />;
}

beforeEach(() => { read.mockReset(); read.mockResolvedValue({ items: [], next_cursor: null }); });
afterEach(async () => {
  cleanup(); useAppearance.setState(appearanceBefore); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

it.each(modes)('keeps the title once and the original list, kind and scope choices in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const user = userEvent.setup();
  show(<Page />);
  expect(screen.getAllByRole('heading', { name: i18n.t('screens.approvals.title'), level: 1 })).toHaveLength(1);
  expect(screen.queryByText(retiredCopy(locale, 'approvals.lead'))).not.toBeInTheDocument();
  expect(await screen.findByText(i18n.t('approvals.empty.waiting.title'))).toBeVisible();
  expect(read).toHaveBeenCalledWith({ scope: 'mine', state: 'pending' });
  expect(read).toHaveBeenCalledWith({scope:'others',state:'pending'});
  expect(screen.getByRole('heading',{name:i18n.t('approvals.others')})).toBeVisible();
  expect(screen.getByRole('tab', { name: i18n.t('approvals.tabs.waiting') })).toHaveAttribute('aria-selected', 'true');
  expect(screen.queryByRole('switch',{name:retiredCopy(locale,'approvals.everything')})).not.toBeInTheDocument();
  await user.click(screen.getByRole('tab', { name: i18n.t('approvals.tabs.history') }));
  await waitFor(() => expect(read).toHaveBeenLastCalledWith({ scope: 'mine', state: 'decided' }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('approvals.kindFilter') }));
  await user.click(screen.getByRole('option', { name: choiceOption(i18n.t, 'approvalKind.access').name }));
  await waitFor(() => expect(read).toHaveBeenLastCalledWith({ scope: 'mine', state: 'decided', kind: 'access' }));
  expect(screen.queryByText(retiredCopy(locale, 'approvals.lead'))).not.toBeInTheDocument();
});
