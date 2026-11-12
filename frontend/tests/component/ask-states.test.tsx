/** Every visible Ask state is named and usable in both shipped languages. */
import { cleanup, fireEvent, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type * as NamedChips from '@/features/people/NamedChip';
import { AskMessageRow } from '@/features/ask/Conversation';
import { AskEmptyState, AskGovernance, AskStatus, AskToolProgress } from '@/features/ask/States';
import { i18n, setLocale } from '@/i18n';
import { askConversation, askMessage } from '../unit/ask-fixtures';
import { show } from './support';

vi.mock('@/features/people/AgentReferences', () => ({ AgentReferences: ({ children }: { children: ReactNode }) => children }));
vi.mock('@/features/people/NamedChip', async (original) => ({ ...await original<typeof NamedChips>(), TypedChip: ({ reference }: { reference: { label: string } }) => <a href="/doc/source">{reference.label}</a> }));
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`names the empty, stopped and streaming states in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale);
    const first = show(<AskEmptyState loading={false} error={null} deleted={false} retry={() => undefined} fresh={() => undefined} />);
    expect(screen.getByText(t('ask.empty'))).toBeVisible();
    first.unmount();
    const stopped = show(<AskStatus conversation={askConversation()} stopped />);
    expect(screen.getByText(t('ask.stopped'))).toBeVisible(); stopped.unmount();
    show(<><AskStatus conversation={askConversation({ state: 'running' })} stopped={false} /><AskMessageRow message={askMessage({ complete: false })} threadId="ask" canRetry={false} onRetry={() => undefined} /></>);
    expect(screen.getByText(t('ask.running'))).toBeVisible();
    expect(screen.getByRole('button', { name: t('ask.retry') })).toBeDisabled();
  });
  it(`renders cited answers with Copy and Retry in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), retry = vi.fn();
    show(<AskMessageRow threadId="ask" canRetry onRetry={retry} message={askMessage({ citations: [{ reference: 'source', url: `${location.origin}/doc/00000000-0000-4000-8000-000000000041`, title: 'Release plan' }] })} />);
    expect(screen.getByRole('list', { name: t('ask.sources') })).toBeVisible();
    expect(screen.getByRole('link', { name: 'Release plan' })).toBeVisible();
    expect(screen.getByRole('button', { name: t('ask.copy') })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: t('ask.retry') })); expect(retry).toHaveBeenCalledOnce();
  });
  it(`shows retention and read-only admin inspection in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale);
    show(<AskGovernance conversation={askConversation({ retention_days: 14, inspecting: true, can: { message: false, keep: false, stop: false, delete: false, export: true } })} />);
    expect(screen.getByText(t('ask.adminNotice'))).toBeVisible();
    expect(screen.getByText(t('ask.inspection'))).toBeVisible();
    expect(screen.getByText(new RegExp(t('ask.retention', { days: '14' })))).toBeVisible();
  });
  it(`names live source reads without showing operation internals in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale);
    show(<><AskToolProgress name="search" /><AskToolProgress name="internal.reader.operation" /></>);
    expect(screen.getByText(t('ask.toolSearch'))).toBeVisible(); expect(screen.getByText(t('ask.toolRead'))).toBeVisible();
    expect(screen.queryByText('internal.reader.operation')).not.toBeInTheDocument();
  });
  it(`offers recovery for refusal and expiry in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), fresh = vi.fn();
    const refused = show(<AskEmptyState loading={false} error={new Error('Access ended')} deleted={false} retry={() => undefined} fresh={fresh} />);
    expect(screen.getByRole('button', { name: t('common.tryAgain') })).toBeEnabled(); refused.unmount();
    show(<AskEmptyState loading={false} error={null} deleted retry={() => undefined} fresh={fresh} />);
    expect(screen.getByText(t('ask.deleted'))).toBeVisible(); fireEvent.click(screen.getByRole('button', { name: t('ask.new') })); expect(fresh).toHaveBeenCalledOnce();
  });
}
