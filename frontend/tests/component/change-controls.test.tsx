/** The wizard says its description, result and completion action once in each catalogue. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';

import { Frame } from '@/features/change/Frame';
import { Result } from '@/features/change/Result';
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
