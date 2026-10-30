/** Program names come from reported declarations, including names of unfamiliar programs. */
import { screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useHarnessTitle } from '@/features/resources/harnesses';
import type { HarnessKindInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
import { agentDescriptor } from '../doubles/agent-descriptor';

const registry = vi.hoisted(() => ({ read: vi.fn<() => Promise<HarnessKindInfo[]>>() }));
vi.mock('@/api/resources', () => ({ harnessKindsQuery: { queryKey: ['declared-programs'], queryFn: registry.read } }));

function Name({ kind }: { kind: string }) {
  const title = useHarnessTitle();
  return <span>{title(kind)}</span>;
}

afterEach(async () => { vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`uses the reported name of a third program in ${locale}`, async () => {
  await setLocale(locale);
  const name = locale === 'en' ? 'Research assistant' : '研究助理';
  const row = { kind: 'research-agent', title: 'Registry title', builds: [], descriptor: agentDescriptor('Descriptor title'), program: null,
    harness: { kind: 'research-agent', name: 'Research assistant', names: { 'zh-CN': '研究助理' } } };
  registry.read.mockResolvedValue([row]);
  show(<Name kind="research-agent" />);
  expect(await screen.findByText(name)).toBeVisible();
  expect(screen.queryByText('research-agent')).not.toBeInTheDocument();
  expect(screen.queryByText('Registry title')).not.toBeInTheDocument();
  expect(screen.queryByText('Descriptor title')).not.toBeInTheDocument();
});

it('does not guess a familiar name while the declaration is missing', async () => {
  registry.read.mockResolvedValue([]);
  show(<Name kind="codex" />);
  expect(await screen.findByText(i18n.t('resources.harnessNameUnknown'))).toBeVisible();
  expect(screen.queryByText('Codex')).not.toBeInTheDocument();
});
