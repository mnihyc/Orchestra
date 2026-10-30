/** UA-07: a request names its question once, keeping answer labels, distinct titles and exact answers. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CardFrame } from '@/features/approvals/Card';
import { AnswersForm, ElicitationForm } from '@/features/approvals/Harness';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, userRef } from '../doubles/fixtures';
import { harnessApproval } from '../doubles/thread-rows';
import { show } from './support';

vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <span>{user.display_name}</span> }));
vi.mock('@/realtime/stores/tree', async (original) => ({ ...await original<object>(), useNodePath: () => ({ path: [] }) }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
const previousTheme = document.documentElement.dataset.theme;
const wasDark = document.documentElement.classList.contains('dark');
const origin = { threadId: 'question-thread', itemId: 'question-item', requester: userRef(me(1, 'ada', 'Ada')) };
afterEach(async () => {
  useAppearance.setState(appearance);
  document.documentElement.classList.toggle('dark', wasDark);
  if (previousTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = previousTheme;
  await setLocale('en');
});

for (const [locale, theme] of modes) {
  it(`names a question once and sends its answer without losing its accessible label (${locale}/${theme})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const question = { id: 'scope', header: '', question: locale === 'en' ? 'Which deployment should change?' : '应更改哪个部署？',
      multiple: false, options: null, allow_other: true };
    const approval = harnessApproval('one-question', { ...origin, title: question.question,
      request: { request_kind: 'user_input', detail: { questions: [question] }, options: [] } });
    const send = vi.fn();
    show(<CardFrame approval={approval}><AnswersForm questions={[question]} disabled={false} pending={false} onSend={send} /></CardFrame>);
    expect(screen.getAllByRole('heading', { name: question.question })).toHaveLength(1);
    expect(screen.getByText(question.question, { selector: 'legend > span' })).toHaveClass('sr-only');
    const answer = screen.getByRole('textbox', { name: question.question });
    const user = userEvent.setup();
    await user.type(answer, 'docs');
    await user.click(screen.getByRole('button', { name: i18n.t('approvals.sendAnswers') }));
    expect(send).toHaveBeenCalledExactlyOnceWith({ scope: ['docs'] });
  });

  it(`retains a distinct request title and labels a choice question only in its legend (${locale}/${theme})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const question = { id: 'target', header: '', question: locale === 'en' ? 'Which target?' : '选择哪个目标？', multiple: false,
      options: [{ label: 'docs', description: '' }, { label: 'app', description: '' }], allow_other: false };
    const title = locale === 'en' ? 'Choose the deployment scope' : '选择部署范围';
    const approval = harnessApproval('choice-question', { ...origin, title,
      request: { request_kind: 'user_input', detail: { questions: [question] }, options: [] } });
    const send = vi.fn();
    show(<CardFrame approval={approval}><AnswersForm questions={[question]} disabled={false} pending={false} onSend={send} /></CardFrame>);
    expect(screen.getByRole('heading', { name: title })).toBeVisible();
    expect(screen.getAllByText(question.question)).toHaveLength(1);
    expect(screen.getByRole('radiogroup', { name: question.question })).toBeVisible();
    const user = userEvent.setup();
    await user.click(screen.getByRole('radio', { name: 'docs' }));
    await user.click(screen.getByRole('button', { name: i18n.t('approvals.sendAnswers') }));
    expect(send).toHaveBeenCalledExactlyOnceWith({ target: ['docs'] });
  });

  it(`keeps an elicitation message in its heading without repeating the body (${locale}/${theme})`, async () => {
    await setLocale(locale);
    useAppearance.setState({ theme, resolvedTheme: theme });
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const message = locale === 'en' ? 'Enter the deployment label' : '输入部署标签';
    const schema = { type: 'object', properties: { label: { type: 'string', title: locale === 'en' ? 'Label' : '标签' } }, required: ['label'] };
    const approval = harnessApproval('elicitation', { ...origin, title: message,
      request: { request_kind: 'mcp_elicitation', detail: { server: 'deployment', message, schema }, options: [] } });
    show(<CardFrame approval={approval}><ElicitationForm schema={schema} message={message} disabled={false} pending={false} onSend={vi.fn()} /></CardFrame>);
    expect(screen.getAllByText(message)).toHaveLength(1);
    expect(screen.getByRole('heading', { name: message })).toBeVisible();
    expect(screen.getByRole('textbox', { name: locale === 'en' ? 'Label' : '标签' })).toBeEnabled();
  });
}
