/** What somebody reads when a limit stands in the way of a turn: for whoever may pass every limit, some, and none; for a cap on turns at once; for a provider's own limit; and for a colleague's message that waits. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';

import type { CapStatus, LimitReached, ProviderRef, ThreadLimits, UsageCap, UserRef } from '@/generated/api-types';
import { bypassOf, kindOf, LimitPrompt } from '@/features/usage/PassLimit';

import { show } from './support';

const ID = '00000000-0000-4000-8003-000000000002';
const OTHER = '00000000-0000-4000-8003-0000000000aa';

function person(name: string, n: number): UserRef {
  return { id: `00000000-0000-4000-8000-${n.toString().padStart(12, '0')}`, kind: 'human', username: name.toLowerCase(), display_name: name, avatar: { text: name.slice(0, 1), color: n } };
}

const PROVIDER: ProviderRef = { id: ID, name: 'Team Pro', kind: 'subscription', harness_kind: null, harness: null, address: 'Acme', placement: 'managed', status: 'ready' };

function cap(over: Partial<UsageCap> = {}): UsageCap {
  return {
    id: ID,
    node: { id: ID, kind: 'project', name: 'Web', redacted: false },
    provider: null,
    subject: { kind: 'everyone' },
    split: 'shared',
    models: [],
    tiers: [],
    harness_kinds: [],
    harnesses: [],
    meter: 'tokens',
    period: 'day',
    counted_from: null,
    amount: '2000000',
    at_limit: 'stop',
    warn_percent: null,
    uncounted: 'allow',
    note: '',
    enabled: true,
    revision: 1,
    created_by: person('Dana', 1),
    created_at: '2026-10-01T09:00:00.000Z',
    updated_at: '2026-10-01T09:00:00.000Z',
    can_edit: false,
    ...over,
  };
}

function status(over: Partial<UsageCap>, used: string, extra: Partial<CapStatus> = {}): CapStatus {
  return {
    cap: cap(over),
    person: null,
    used,
    in_progress: '0',
    waiting: 0,
    left_out: { unrated_tokens: 0, unmetered_threads: 0 },
    state: 'reached',
    period_ends_at: '2026-10-13T00:00:00.000Z',
    replaced_by: null,
    can_bypass: false,
    bypasses: { count: 0, last: null },
    ...extra,
  };
}

function limit(over: Partial<LimitReached> = {}): LimitReached {
  return { ask: [], ask_more: 0, can_bypass: false, cap: status({}, '2140000'), passed_by: null, provider_limit: null, ...over };
}

/** A cap on turns at once, on a provider, with four running of four. */
function running(): CapStatus {
  return status({ meter: 'running', period: null, amount: '4', provider: PROVIDER, node: null }, '4', { period_ends_at: null, waiting: 2 });
}

function prompt(limits: ThreadLimits, threadId?: string) {
  const onSend = vi.fn();
  show(<LimitPrompt open onOpenChange={() => undefined} limits={limits} onSend={onSend} pending={false} threadId={threadId} />);
  return { onSend, dialog: screen.getByRole('dialog', { name: 'This would go past a limit' }) };
}

describe('the limit prompt', () => {
  it('shows each limit in the words of its cap, with what is used of how much, when it starts again and who set it', () => {
    const { dialog } = prompt({ bypass_all: false, reached: [limit()] });
    expect(within(dialog).getByText('Everyone together on all models in project Web')).toBeInTheDocument();
    expect(within(dialog).getByText('2.1M of 2M tokens')).toBeInTheDocument();
    expect(within(dialog).getByText('starts again 00:00 UTC')).toBeInTheDocument();
    const setter = within(dialog).getByText('Set by');
    expect(setter).toBeVisible();
    const author = within(setter).getByRole('button', { name: 'Dana' });
    expect(author).toBeVisible();
    expect(within(author).getByText('Dana')).toBeVisible();
    expect(within(dialog).getByRole('meter')).toHaveAttribute('aria-valuenow', '100');
  });

  it('lets somebody who may pass every limit proceed, naming the ones they pass, and says what proceeding comes to', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({ bypass_all: true, reached: [limit({ can_bypass: true }), limit({ can_bypass: true, cap: status({ id: OTHER, period: 'week' }, '9000000') })] });
    expect(within(dialog).getByText('This turn will run and be counted. The limits stay as they are, and it is recorded that you let it past them.')).toBeInTheDocument();
    expect(within(dialog).getAllByText('You may let work past this limit.')).toHaveLength(2);
    expect(within(dialog).queryByRole('button', { name: 'Send and wait' })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Proceed anyway' }));
    expect(onSend).toHaveBeenCalledWith({ caps: [ID, OTHER] });
  });

  it('gives somebody who may pass some of them and not the rest "Pass mine and wait", and says which is which', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({
      bypass_all: false,
      reached: [limit({ can_bypass: true }), limit({ cap: status({ id: OTHER, node: { id: OTHER, kind: 'org', name: 'Acme', redacted: false }, period: 'week' }, '3000000'), ask: [person('Erin', 2), person('Fay', 3)], ask_more: 3 })],
    });
    expect(within(dialog).queryByRole('button', { name: 'Proceed anyway' })).not.toBeInTheDocument();
    expect(within(dialog).getByText('You may let this turn past some of these. The others are for the people named under them; the message waits for them.')).toBeInTheDocument();
    expect(within(dialog).getByText('Some limits in the way are not yours to pass: the message will wait for the others to be let through.')).toBeInTheDocument();
    expect(within(dialog).getByText('You may let work past this limit.')).toBeInTheDocument();
    expect(within(dialog).getByText('Who can let it through: Erin, Fay · 3 more')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Pass mine and wait' }));
    expect(onSend).toHaveBeenCalledWith({ caps: [ID] });
  });

  it('gives somebody who may pass none the limits and who could let them through, and lets them send and wait', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({ bypass_all: false, reached: [limit({ ask: [person('Dana', 1), person('Erin', 2)] })] });
    expect(within(dialog).getByText('The message waits until the limit starts over, or until somebody lets it through.')).toBeInTheDocument();
    expect(within(dialog).getByText('Who can let it through: Dana, Erin')).toBeInTheDocument();
    expect(within(dialog).queryByRole('button', { name: /Proceed|Pass mine/ })).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Send and wait' }));
    expect(onSend).toHaveBeenCalledWith(undefined);
  });

  it('says that nobody can let a turn through where nobody holds the row', () => {
    const { dialog } = prompt({ bypass_all: false, reached: [limit()] });
    expect(within(dialog).getByText('Nobody can let it through from here.')).toBeInTheDocument();
  });

  it('says how many turns run against a cap on turns at once, and starts the turn now for whoever may pass it', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({ bypass_all: true, reached: [limit({ cap: running(), can_bypass: true })] });
    expect(within(dialog).getByText('4 of 4 turns are running on Team Pro. Yours would wait its turn.')).toBeInTheDocument();
    expect(within(dialog).queryByRole('meter')).not.toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Start it now anyway' }));
    expect(onSend).toHaveBeenCalledWith({ caps: [ID] });
  });

  it('lets somebody who may not pass a cap on turns at once wait their turn, and says that nothing running is stopped', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({ bypass_all: false, reached: [limit({ cap: running() })] });
    expect(within(dialog).getByText('Your turn waits its turn. The limit stops nothing that is running.')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Wait' }));
    expect(onSend).toHaveBeenCalledWith(undefined);
  });

  it('says that a provider has reported its own limit as reached, how long ago, and offers to try anyway', async () => {
    const user = userEvent.setup();
    const reported = new Date(Date.now() - 12 * 60_000).toISOString();
    const { onSend, dialog } = prompt({ bypass_all: true, reached: [limit({ cap: null, provider_limit: { as_of: reported, provider: PROVIDER, resets_at: null, window: '5-hour' }, can_bypass: true })] });
    expect(within(dialog).getByText('The plan limit of Team Pro')).toBeInTheDocument();
    expect(within(dialog).getByText('Team Pro reported its 5-hour limit as reached, 12 min. ago. It may refuse this turn.')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Try anyway' }));
    expect(onSend).toHaveBeenCalledWith({ provider_limit: true });
  });

  it('shows a colleague\'s waiting message with what is passed already and by whom, and passes only what is left', async () => {
    const user = userEvent.setup();
    const { onSend, dialog } = prompt({
      bypass_all: true,
      reached: [limit({ passed_by: person('Dana', 1), can_bypass: true }), limit({ can_bypass: true, cap: status({ id: OTHER, period: 'week' }, '9000000') })],
    });
    expect(within(dialog).getByText('Let past by Dana.')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: 'Proceed anyway' }));
    expect(onSend).toHaveBeenCalledWith({ caps: [OTHER] });
  });

  it('offers to change a limit from a thread to whoever may edit the cap, and to nobody else', () => {
    const editable = { bypass_all: false, reached: [limit({ cap: status({ can_edit: true }, '2140000') })] };
    prompt(editable, ID);
    expect(screen.getByRole('button', { name: 'Change a limit…' })).toBeInTheDocument();
  });

  it('does not offer to change a limit that the person may not edit', () => {
    prompt({ bypass_all: false, reached: [limit()] }, ID);
    expect(screen.queryByRole('button', { name: 'Change a limit…' })).not.toBeInTheDocument();
  });
});

describe('what a person lets past', () => {
  it('is the caps they hold the row for that nobody has let past, and the provider\'s limit where they may', () => {
    expect(bypassOf({ bypass_all: false, reached: [] })).toBeUndefined();
    expect(bypassOf({ bypass_all: false, reached: [limit()] })).toBeUndefined();
    expect(bypassOf({ bypass_all: true, reached: [limit({ can_bypass: true })] })).toEqual({ caps: [ID] });
    expect(bypassOf({ bypass_all: true, reached: [limit({ can_bypass: true, passed_by: person('Dana', 1) })] })).toBeUndefined();
    expect(bypassOf({ bypass_all: true, reached: [limit({ cap: null, can_bypass: true, provider_limit: { as_of: '2026-10-12T10:00:00.000Z', provider: PROVIDER, resets_at: null, window: 'weekly' } })] })).toEqual({ provider_limit: true });
  });

  it('is named by the kind of limit in the way: all on turns at once, the provider\'s own, or a mixture', () => {
    const own = limit({ cap: null, provider_limit: { as_of: '2026-10-12T10:00:00.000Z', provider: PROVIDER, resets_at: null, window: 'weekly' } });
    expect(kindOf({ bypass_all: false, reached: [limit({ cap: running() })] })).toBe('running');
    expect(kindOf({ bypass_all: false, reached: [own] })).toBe('provider');
    expect(kindOf({ bypass_all: false, reached: [limit()] })).toBe('mixed');
    expect(kindOf({ bypass_all: false, reached: [limit({ cap: running() }), own] })).toBe('mixed');
  });
});
