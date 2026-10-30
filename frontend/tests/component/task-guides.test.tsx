/** One question mark opens the task, keeps hidden views out, and renders an authored localized article. */
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useTranslation } from 'react-i18next';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ArticleView } from '@/features/help/Article';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { articleInLanguage } from '@/help/guide';
import { guide } from '@/help/library';
import { TASK_GUIDES, type TaskGuideId } from '@/help/tasks';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const open = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => open }));
beforeEach(() => { open.mockClear(); });
afterEach(async () => { await setLocale('en'); });

describe('task question marks', () => {
  it.each(['en', 'zh-CN'] as const)('names and opens every task directly without an intermediate hint (%s)', async (language) => {
    await setLocale(language);
    const user = userEvent.setup();
    show(<>{TASK_GUIDES.map((task) => <TaskGuideLink key={task.id} guide={task.id} />)}</>);
    for (const task of TASK_GUIDES) {
      const label = i18n.t('taskGuides.open', { title: i18n.t(task.title) });
      const button = screen.getByRole('button', { name: label });
      expect(screen.getAllByRole('button', { name: label })).toHaveLength(1);
      expect(button).toHaveAttribute('data-help-article', task.id);
      expect(button.textContent.trim()).toBe('');
      await user.click(button);
      expect(open).toHaveBeenLastCalledWith(task.id);
    }
    expect(open).toHaveBeenCalledTimes(TASK_GUIDES.length);
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });

  it('keeps a single visible entry and restores another when its first row leaves', async () => {
    const first = show(<TaskGuideLink guide="temporary-environments" />);
    show(<><div hidden><TaskGuideLink guide="temporary-environments" /></div><TaskGuideLink guide="temporary-environments" /></>);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Help: Temporary environments' })).toHaveLength(1));
    first.unmount();
    const button = await screen.findByRole('button', { name: 'Help: Temporary environments' });
    button.focus();
    await userEvent.keyboard('{Enter}');
    expect(open).toHaveBeenCalledExactlyOnceWith('temporary-environments');
  });
});

function Reading({ id }: { id: TaskGuideId }) {
  const { i18n: language } = useTranslation();
  const source = guide.articles.find((one) => one.id === id);
  if (source === undefined) throw new Error(`missing task guide: ${id}`);
  return <ArticleView article={articleInLanguage(source, language.resolvedLanguage ?? language.language)} heading={null} onArticle={vi.fn()} onScreen={vi.fn()} />;
}

it.each(TASK_GUIDES)('renders $id in Chinese after a language change, with one article heading', async (task) => {
  show(<Reading id={task.id} />);
  expect(screen.getAllByRole('heading', { level: 1, name: i18n.t(task.title) })).toHaveLength(1);
  await act(async () => { await setLocale('zh-CN'); });
  expect(screen.getAllByRole('heading', { level: 1, name: i18n.t(task.title) })).toHaveLength(1);
  expect(screen.getByRole('region', { name: i18n.t(task.title) })).toHaveTextContent(/\p{Script=Han}/u);
});
