/** Repeated cards contribute one real guide entry, with a named fallback when the first view disappears. */
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { CreationPermissionLink } from '@/features/help/CreationPermissionLink';
import { GuideLink } from '@/features/help/GuideLink';
import { ProviderGuideLink } from '@/features/help/ProviderGuideLink';
import { i18n, setLocale } from '@/i18n';
import { useGuideEntry } from '@/lib/guideEntries';
import { Popover } from '@/ui/Popover';
import { show } from './support';

const calls = vi.hoisted(() => ({ open: vi.fn(), access: vi.fn() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => calls.open }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: calls.access }));

describe('one guide entry on the screen', () => {
  it('keeps an explanation open when its own floating card offers the same guide destination', async () => {
    function Explanation() {
      const entry = useGuideEntry('nested-explanation', 'section');
      return <span ref={entry.ref}>{!entry.hidden && <Popover label="Explanation" trigger={<button>Open explanation</button>}>
        <ArticleGuideLink article="nested-explanation" heading="section" title="Read this guide" />
      </Popover>}</span>;
    }
    show(<Explanation />);
    await userEvent.click(screen.getByRole('button', { name: 'Open explanation' }));
    expect(await screen.findByRole('dialog', { name: 'Explanation' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Read this guide' })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Open explanation' })).toBeVisible();
  });
  it('does not re-read every guide style for unrelated floating motion, while a hidden owner still promotes its fallback', async () => {
    function Entry() {
      const entry = useGuideEntry('contextual-guide', 'section');
      return <span ref={entry.ref}>{!entry.hidden && <button>Contextual guide</button>}</span>;
    }
    render(<><section data-testid="guide-first"><Entry /></section><section data-testid="guide-second"><Entry /></section><div data-testid="floating-motion" /></>);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'Contextual guide' })).toHaveLength(1));
    await act(async () => { await Promise.resolve(); });
    const styles = vi.spyOn(globalThis, 'getComputedStyle');
    try {
      await act(async () => { screen.getByTestId('floating-motion').style.transform = 'translateX(4px)'; await Promise.resolve(); });
      expect(styles).not.toHaveBeenCalled();
      await act(async () => { screen.getByTestId('guide-first').hidden = true; await Promise.resolve(); });
      await waitFor(() => expect(screen.getByTestId('guide-second').querySelector('button')).not.toBeNull());
      expect(styles).toHaveBeenCalled();
      expect(screen.getAllByRole('button', { name: 'Contextual guide' })).toHaveLength(1);
    } finally { styles.mockRestore(); }
  });
  it('names the different guides instead of repeating How this works, in both languages', async () => {
    for (const locale of ['en', 'zh-CN'] as const) {
      await setLocale(locale);
      const view = show(<><GuideLink guide="let-agents-open-ports" /><GuideLink guide="change-a-running-environment" /><ProviderGuideLink guide="commands-after-start" /></>);
      for (const key of ['helpRelease.guides.ports', 'helpRelease.guides.liveEnvironment', 'helpProvider.commands'] as const) expect(screen.getByRole('button', { name: i18n.t(key) })).toHaveTextContent(i18n.t(key));
      expect(screen.getAllByRole('button')).toHaveLength(3);
      view.unmount();
    }
    await setLocale('en');
  });

  it('coalesces repeated cards, then restores the surviving entry when the first is removed', async () => {
    const first = show(<GuideLink guide="how-approvals-work" />);
    show(<GuideLink guide="how-approvals-work" />);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'How approvals work' })).toHaveLength(1));
    first.unmount();
    const entry = await screen.findByRole('button', { name: 'How approvals work' });
    await userEvent.click(entry);
    expect(calls.open).toHaveBeenCalledWith('how-approvals-work');
    expect(screen.getAllByRole('button', { name: 'How approvals work' })).toHaveLength(1);
  });

  it('inactive tabs do not take the entry from the visible screen', async () => {
    const view = show(<><div hidden><GuideLink guide="how-approvals-work" /></div><GuideLink guide="how-approvals-work" /></>);
    expect(screen.getByRole('button', { name: 'How approvals work' })).toBeVisible();
    view.rerender(<><div><GuideLink guide="how-approvals-work" /></div><GuideLink guide="how-approvals-work" /></>);
    await waitFor(() => expect(screen.getAllByRole('button', { name: 'How approvals work' })).toHaveLength(1));
  });

  it('keeps separate anchors and opens the actual section', async () => {
    show(<><ArticleGuideLink article="resources" heading="templates" title="Templates guide" /><ArticleGuideLink article="resources" heading="mounts" title="Mounts guide" /></>);
    await userEvent.click(screen.getByRole('button', { name: 'Templates guide' }));
    expect(calls.open).toHaveBeenCalledWith('resources', 'templates');
    expect(screen.getByRole('button', { name: 'Mounts guide' })).toBeVisible();
  });
});

describe('the empty place’s creation question', () => {
  it.each(['org', 'project'] as const)('opens %s’s own creation row without an access write', async (kind) => {
    calls.access.mockClear();
    show(<CreationPermissionLink place={{ kind, id: 'actual-place', name: 'My place' }} />);
    await userEvent.click(screen.getByRole('button', { name: kind === 'org' ? 'Who may make projects here?' : 'Who may open topics here?' }));
    expect(calls.access).toHaveBeenCalledWith({ kind, id: 'actual-place', name: 'My place' }, { share: false, permission: kind === 'project' ? 'project.create_topics' : 'org.interact' });
  });
});
