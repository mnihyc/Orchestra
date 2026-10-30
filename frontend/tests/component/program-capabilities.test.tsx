/** Declared capabilities keep their names, states and reasons in both interface languages. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it } from 'vitest';
import { ProgramCapabilities, type ProgramCapabilityGroups } from '@/features/admin/ProgramCapabilities';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`shows declared capabilities and the reason an extra is off (${locale})`, async () => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Research planning' : '研究规划';
  const reason = locale === 'en' ? 'This build did not pass its planning check.' : '此版本未通过规划检查。';
  const titles = locale === 'en'
    ? { conversation: 'Conversation', tools: 'Tools', sessions: 'Sessions', usage: 'Usage' }
    : { conversation: '对话', tools: '工具', sessions: '会话', usage: '用量' };
  const groups: ProgramCapabilityGroups = [
    { id: 'conversation', title: titles.conversation, items: [{ id: 'plugin.plan', title: name, available: false, reason }] },
    { id: 'tools', title: titles.tools, items: [{ id: 'plugin.inspect', title: locale === 'en' ? 'Inspect data' : '检查数据', available: true }] },
    { id: 'sessions', title: titles.sessions, items: [] },
    { id: 'usage', title: titles.usage, items: [] },
  ];
  show(<ProgramCapabilities groups={groups} />);
  const conversation = screen.getByRole('region', { name: titles.conversation });
  expect(within(conversation).getByText(i18n.t('programCapabilities.state', { name, state: i18n.t('programCapabilities.off') }))).toBeVisible();
  const tools = screen.getByRole('region', { name: titles.tools });
  expect(within(tools).getByText(i18n.t('programCapabilities.state', { name: groups[1]!.items[0]!.title, state: i18n.t('programCapabilities.on') }))).toBeVisible();
  expect(screen.queryByText('plugin.plan')).not.toBeInTheDocument();
  await userEvent.click(within(conversation).getByRole('button', { name: i18n.t('help.hint.label', { term: name }) }));
  expect(screen.getByRole('dialog', { name })).toHaveTextContent(reason);
  await userEvent.keyboard('{Escape}');
  expect(screen.queryByRole('dialog', { name })).not.toBeInTheDocument();
});

it('distinguishes an absent descriptor from a reported one with no declarations', () => {
  const view = show(<ProgramCapabilities groups={null} />);
  expect(screen.getByText(i18n.t('programCapabilities.notReported'))).toBeVisible();
  view.rerender(<ProgramCapabilities groups={['Conversation', 'Tools', 'Sessions', 'Usage'].map((title) => ({ id: title, title, items: [] }))} />);
  expect(screen.queryByText(i18n.t('programCapabilities.notReported'))).not.toBeInTheDocument();
  expect(screen.getAllByText(i18n.t('programCapabilities.empty'))).toHaveLength(4);
});

it('renders a new declared group without a frontend group registry', () => {
  show(<ProgramCapabilities groups={[{ id: 'analysis-extra', title: 'Research workflow', items: [{ id: 'plugin.research', title: 'Cited research', available: true }] }]} />);
  expect(screen.getByRole('region', { name: 'Research workflow' })).toHaveTextContent('Cited research');
});
