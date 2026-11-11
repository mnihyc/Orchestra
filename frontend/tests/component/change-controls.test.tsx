/** The wizard says its description, result and completion action once in each catalogue. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { Frame } from '@/features/change/Frame';
import { Options } from '@/features/change/Options';
import { Result } from '@/features/change/Result';
import { initialChoices } from '@/features/change/choices';
import type { Job } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Button } from '@/ui/Button';

import { show } from './support';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en','zh-CN'] as const) it(`keeps one description, one outcome and one completion action (${locale})`,async () => {
  await setLocale(locale);
  const t=i18n.getFixedT(locale);
  const about=t('changeCompact.about.switch');
  const outcome=locale==='en'?'This thread now runs on Readable Model.':'此会话现在使用 Readable Model。';
  const complete=locale==='en'?'Done':'完成';
  const job={state:'succeeded',log:null,error:null,progress:{phase:'done',phases:[],percent:100}} as unknown as Job;
  show(<Frame title={locale==='en'?'Switch “Thread”':'切换“会话”'} about={about} at="done" open onClose={vi.fn()} footer={<Button>{complete}</Button>}>
    <Result job={job} done={outcome} stoppedTurn={false} waited={0} subject="Thread" />
  </Frame>);
  expect(screen.getAllByText(about,{exact:true})).toHaveLength(1);
  expect(screen.getAllByText(outcome,{exact:true})).toHaveLength(1);
  expect(screen.getAllByText(complete,{exact:true})).toHaveLength(1);
  expect(screen.getByText(t('change.steps.done'),{exact:true})).toBeVisible();
});

it('keeps fork access choices concise and explains them in field help', async () => {
  await setLocale('en');
  const t = i18n.getFixedT('en');
  const user = userEvent.setup();
  const onChange = vi.fn();
  show(<Options choices={initialChoices('new')} onChange={onChange} applied={null} plan={null} topic="Fix login" />);

  const label = t('fork.page.title');
  const detail = t('fork.page.privateAbout');
  const group = screen.getByRole('radiogroup', { name: label });
  expect(within(group).queryByText(detail, { exact: true })).not.toBeInTheDocument();

  await user.click(screen.getByRole('button', { name: t('help.hint.label', { term: label }) }));
  const help = screen.getByRole('dialog', { name: label });
  expect(within(help).getByText(detail, { exact: true })).toBeVisible();
  await user.keyboard('{Escape}');

  await user.click(within(group).getByRole('radio', { name: t('changeCompact.access.private') }));
  expect(onChange).toHaveBeenCalledWith({ page: 'private' });
});
