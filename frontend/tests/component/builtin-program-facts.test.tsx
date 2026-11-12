/** A declared built-in program is ready without a native profile or environment. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { BindingSummary } from '@/features/thread/BindingSummary';
import { ComposerControls } from '@/features/composer/Controls';
import type { Thread } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`describes the actual program without invented native binding facts (${locale})`, async () => {
  // EN: open an empty built-in thread > read its program and No environment
  // needed > Fork > review the Private copy. No native setup is needed.
  // 中文：打开空的内置线程 > 查看只读问答及无需环境说明 > 分叉 > 查看私有副本，无需原生配置。
  await setLocale(locale);
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('The source needs its genuine thread');
  const thread: Thread = { ...world.threads.thread(person, row), binding: null, environment: null,
    program: { id: 'orch', root_org_id: ACME.org, read_only: true, requires_environment: false, requires_native_harness: false } };
  show(<BindingSummary thread={thread} />);
  expect(screen.getByText(i18n.t('ask.threadProgram'))).toBeVisible();
  expect(screen.getByText(i18n.t('ask.threadProgramDescription'))).toBeVisible();
  expect(screen.getByText(i18n.t('ask.noEnvironment'))).toBeVisible();
  expect(screen.queryByText(i18n.t('thread.environment.none'))).not.toBeInTheDocument();
  cleanup();
  show(<ComposerControls thread={thread} attachment={<button>Attach</button>} delivery={<button>Send question</button>} />);
  expect(screen.getByText(i18n.t('ask.threadProgram'))).toBeVisible();
  expect(screen.queryByText(i18n.t('composer.row.unbound'))).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Attach' })).toBeVisible();
  expect(screen.getByRole('button', { name: 'Send question' })).toBeVisible();
});
