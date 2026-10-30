/** A section offers one question; opening its actual guide closes the question and retains its source target. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { GuideLink } from '@/features/help/GuideLink';
import { GuideRail } from '@/features/help/GuideRail';
import { useHelpState } from '@/features/help/state';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
vi.mock('@/features/workbench/Narrow', () => ({ useShape: () => 'wide' }));
afterEach(async () => { document.documentElement.removeAttribute('data-theme'); calls.open.mockClear(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const article of ['let-agents-open-ports', 'change-a-running-environment'] as const) {
  it(`offers ${article} behind the section's single question (${locale}/${theme})`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const target = { kind: 'environment', id: 'current-environment' } as const;
    show(<GuideRail label={i18n.t('egress.title')}><GuideLink guide="let-agents-open-ports" target={target} /><GuideLink guide="change-a-running-environment" target={target} /></GuideRail>);
    expect(screen.getAllByRole('button')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('egress.title') }) }));
    expect(screen.getByText(i18n.t('helpFlow.guides'))).toBeVisible();
    const title = i18n.t(article === 'let-agents-open-ports' ? 'helpRelease.guides.ports' : 'helpRelease.guides.liveEnvironment');
    await userEvent.click(screen.getByRole('button', { name: title }));
    expect(calls.open).toHaveBeenCalledWith(article);
    expect(useHelpState.getState().guideContext).toEqual({ article, target });
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(screen.getAllByRole('button')).toHaveLength(1);
  });
}
