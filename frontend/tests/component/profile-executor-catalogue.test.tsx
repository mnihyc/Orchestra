/** Profile details use declared names without a read per binding or an unbounded DOM list. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { Bindings } from '@/features/resources/agents/Bindings';
import type { ArrangementOffer } from '@/features/resources/agents/descriptor';
import { DETAIL_PAGE_ROWS } from '@/features/resources/DetailSections';
import type { ExecutorInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

const { read } = vi.hoisted(() => ({ read: vi.fn<() => Promise<ExecutorInfo[]>>() }));
vi.mock('@/api/executors', () => ({ executorCatalogueQuery: { queryKey: ['profile-executor-catalogue'], queryFn: read } }));

const declared = { kind: 'research-exec', title: { en: 'Research executor', 'zh-CN': '研究执行器' }, program: null, offers: [],
  tool_sets: [{ name: 'research-tools', title: { en: 'Research tools', 'zh-CN': '研究工具' }, summary: { en: 'Tools for research' } }] };
const arrangements = [{ type: 'separated', executor: 'research-exec', toolSets: ['research-tools'] }];
beforeEach(() => { vi.clearAllMocks(); read.mockResolvedValue([declared]); });

for (const locale of ['en', 'zh-CN'] as const) it(`names a shared executor from its catalogue (${locale})`, async () => {
  await setLocale(locale);
  try {
    show(<Bindings arrangements={arrangements} program={null} />);
    expect(await screen.findByText(i18n.t('agents.overview.executor', { executor: locale === 'en' ? 'Research executor' : '研究执行器' }))).toBeVisible();
    expect(screen.getByText(i18n.t('agents.overview.toolSets', { list: locale === 'en' ? 'Research tools' : '研究工具' }))).toBeVisible();
    expect(document.body).not.toHaveTextContent('research-exec');
    expect(document.body).not.toHaveTextContent('research-tools');
    expect(read).toHaveBeenCalledOnce();
  } finally { await setLocale('en'); }
});

it('uses the program declaration without fetching a second catalogue', () => {
  show(<Bindings arrangements={arrangements} program={{ executors: [declared] }} />);
  expect(screen.getByText(i18n.t('agents.overview.executor', { executor: 'Research executor' }))).toBeVisible();
  expect(screen.getByText(i18n.t('agents.overview.toolSets', { list: 'Research tools' }))).toBeVisible();
  expect(read).not.toHaveBeenCalled();
});

it('names a pending read and retries a failed read without inventing an executor name', async () => {
  read.mockRejectedValueOnce(new NetworkError('failed'));
  show(<Bindings arrangements={arrangements} program={null} />);
  expect(screen.getByText(i18n.t('agents.overview.loadingExecutors'))).toBeVisible();
  expect(await screen.findByText(i18n.t('machines.policy.executorReadFailed'))).toBeVisible();
  expect(document.body).not.toHaveTextContent('research-exec');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByText(i18n.t('agents.overview.executor', { executor: 'Research executor' }))).toBeVisible();
  expect(read).toHaveBeenCalledTimes(2);
});

it('keeps an unreported name unavailable after an empty catalogue answer', async () => {
  read.mockResolvedValue([]);
  show(<Bindings arrangements={arrangements} program={null} />);
  expect(await screen.findByText(i18n.t('agents.overview.executor', { executor: i18n.t('ui.nameUnknown') }))).toBeVisible();
  expect(document.body).not.toHaveTextContent('research-exec');
  expect(document.body).not.toHaveTextContent('research-tools');
});

it('mounts only twelve of a hundred thousand arrangements and reads one catalogue across pages', async () => {
  const offers: ArrangementOffer[] = Array.from({ length: 100_000 }, (_, index) => ({ type: `arrangement-${index}`, executor: 'research-exec', toolSets: [] }));
  const view = show(<Bindings arrangements={offers} program={null} />);
  await screen.findAllByText(i18n.t('agents.overview.executor', { executor: 'Research executor' }));
  const user = userEvent.setup();
  for (let page = 0; page < 4; page += 1) {
    expect(view.container.querySelectorAll('[data-agent-arrangement]')).toHaveLength(DETAIL_PAGE_ROWS);
    expect(read).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  }
});

it('keeps a hundred thousand declared tool sets within one page of visible names', async () => {
  const toolSets = Array.from({ length: 100_000 }, (_, index) => ({ name: `tools-${index}`, title: { en: `Tool set ${index}` }, summary: { en: 'Declared tools' } }));
  show(<Bindings arrangements={[{ type: 'separated', executor: 'research-exec', toolSets: toolSets.map((entry) => entry.name) }]} program={{ executors: [{ ...declared, tool_sets: toolSets }] }} />);
  const prefix = i18n.t('agents.overview.toolSets', { list: '' });
  const line = () => screen.getByText((text) => text.startsWith(prefix));
  expect(line()).toHaveTextContent('Tool set 0');
  expect(line()).toHaveTextContent(`Tool set ${DETAIL_PAGE_ROWS - 1}`);
  expect(line()).not.toHaveTextContent(`Tool set ${DETAIL_PAGE_ROWS},`);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  expect(line()).toHaveTextContent(`Tool set ${DETAIL_PAGE_ROWS}`);
  expect(line()).not.toHaveTextContent('Tool set 0');
  expect(line().textContent.length).toBeLessThan(500);
  expect(read).not.toHaveBeenCalled();
});
