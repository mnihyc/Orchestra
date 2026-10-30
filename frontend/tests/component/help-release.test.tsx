/** What the reader sees: separate help examples, fresh authorized sources, and a frame that leaves the control usable. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { SettingEntry } from '@/features/help/SettingEntry';
import type { SettingDef } from '@/generated/api-types';
import { Spotlight, useSpotlightTarget } from '@/ui/Spotlight';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), targetSettingsQuery: () => ({ queryKey: ['help-source'], queryFn: () => Promise.reject(new Error('Access was removed')) }) }));

const expiry: SettingDef = {
  key: 'requests.expiry_hours', title: 'Request expiry', description: 'How long an agent boundary request waits.', group: 'confinement', class: 'policy', scopes: ['system', 'org', 'project', 'topic'], merge: 'min',
  default: 24, example: 12, schema: { type: 'integer', minimum: 1, maximum: 168 }, text_key: null, keywords: [],
};

describe('a setting entry', () => {
  it('shows an example separately from its shipped value and links its relevant guide', () => {
    show(<SettingEntry def={expiry} onBack={() => undefined} />);
    expect(screen.getByRole('heading', { name: 'Shipped value' }).nextElementSibling).toHaveTextContent('24');
    expect(screen.getByRole('heading', { name: 'Example value' }).nextElementSibling).toHaveTextContent('12');
    expect(screen.getByRole('button', { name: 'How approvals work' })).toBeVisible();
  });
  it.each([
    { key: 'agents.subagents_at_once', shipped: 8, example: 4, maximum: 64 },
    { key: 'agents.subagent_depth', shipped: 1, example: 1, maximum: 5 },
  ])('explains the org-wide four-agent example and the shipped value for $key', ({ key, shipped, example, maximum }) => {
    const def: SettingDef = { ...expiry, key, title: 'Sub-agent bound', description: 'A bound passed to the harness.', class: 'parameter',
      scopes: ['system', 'org', 'project', 'topic', 'thread'], default: shipped, example, schema: { type: 'integer', minimum: 1, maximum } };
    show(<SettingEntry def={def} onBack={() => undefined} />);
    expect(screen.getByRole('heading', { name: 'Shipped value' }).nextElementSibling).toHaveTextContent(String(shipped));
    expect(screen.getByRole('heading', { name: 'Example value' }).nextElementSibling).toHaveTextContent(String(example));
    expect(screen.getByText(/Set agents\.subagents_at_once to 4 and agents\.subagent_depth to 1 on an org/)).toHaveTextContent('those sub-agents start none of their own');
    expect(screen.getByText(/The shipped values are 8 and 1/)).toHaveTextContent('a larger one cannot relax it');
  });
  it.each([
    { key: 'mcp.enabled', words: /Browser tools fails with exit 1/, outcome: 'other servers and running turns continue', shipped: {}, example: { '00000000-0000-4000-8000-000000000001': true } },
    { key: 'capability.recreate_grace_secs', words: /a turn is running and capability\.recreate_grace_secs is 120/, outcome: 'keeping every file, including /opt/restart-proof.txt outside the volumes', shipped: 120, example: 30 },
  ])('explains what restarts, what is kept and how the job reports its result for $key', ({ key, words, outcome, shipped, example }) => {
    const def: SettingDef = { ...expiry, key, default: shipped, example };
    show(<SettingEntry def={def} onBack={() => undefined} />);
    expect(screen.getByText(words)).toHaveTextContent(outcome);
    expect(screen.getByText(words)).toHaveTextContent(/job/i);
    expect(screen.getByRole('button', { name: 'Change an environment while it runs' })).toBeVisible();
  });
  it('hides an old cached private source while rechecking and after the read is refused', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    cache.setQueryData(['help-source'], [{ key: expiry.key, source: { scope: 'project', node: { name: 'Old private source', id: 'old', kind: 'project', redacted: false } }, parts: [], locked_by: null }]);
    show(<QueryClientProvider client={cache}><SettingEntry def={expiry} target={{ kind: 'node', id: 'thread-1' }} onBack={() => undefined} /></QueryClientProvider>);
    expect(screen.queryByText('Old private source')).not.toBeInTheDocument();
    expect(await screen.findByRole('button', { name: 'Try again' })).toBeInTheDocument();
    expect(screen.queryByText('Old private source')).not.toBeInTheDocument();
  });
});

describe('the real control frame', () => {
  beforeEach(() => { Element.prototype.scrollIntoView = () => undefined; });
  function Frame() { return <Spotlight target={useSpotlightTarget('[data-test-control]')} />; }
  function rect(left: number): DOMRect { return new DOMRect(left, 20, 80, 30); }
  it('follows movement, ignores a hidden copy, and leaves the actual button operable', async () => {
    let left = 30;
    vi.spyOn(Element.prototype, 'getClientRects').mockImplementation(function (this: Element) { return this.closest('[hidden]') === null ? [rect(left)] as unknown as DOMRectList : [] as unknown as DOMRectList; });
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(() => rect(left));
    const click = vi.fn();
    show(<><div hidden><button data-test-control type="button">Hidden copy</button></div><button data-test-control type="button" onClick={click}>The real control</button><Frame /></>);
    await waitFor(() => expect(screen.getByTestId('tutorial-frame')).toHaveStyle({ left: '26px', top: '16px' }));
    await userEvent.click(screen.getByRole('button', { name: 'The real control' }));
    expect(click).toHaveBeenCalledTimes(1);
    left = 70;
    act(() => { window.dispatchEvent(new Event('resize')); });
    await waitFor(() => expect(screen.getByTestId('tutorial-frame')).toHaveStyle({ left: '66px' }));
    vi.restoreAllMocks();
  });
});
