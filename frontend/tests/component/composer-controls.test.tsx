/** Message controls retain their reading order; watch-only readers get a notice without driving controls. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { ComposerControls, ReadOnlyComposerControls, ThreadRunsLine } from '@/features/composer/Controls';
import { i18n, setLocale } from '@/i18n';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { provider as providerChoice } from '../unit/start-fixtures';
import type { Thread } from '@/generated/api-types';
import { show } from './support';

const change = vi.hoisted(() => vi.fn());
vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: change }));
vi.mock('@/features/start/SetUpThread', () => ({ SetUpThread: () => <div role="dialog" aria-label="Set up thread" /> }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Model and effort</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Approval mode</button>, ConfinementPicker: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Sandbox</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context breakdown</button> }));
vi.mock('@/features/composer/runsWith', async (loadOriginal) => ({ ...await loadOriginal<object>(), useRunsWith: () => ({ parameters: undefined, states: [] }) }));

const thread = { id: 'thread-1', name: 'Repair login', archived: false, state: 'idle', side_of: null, lock: null,
  owner: { display_name: 'Ada' }, binding: { agent: { id: 'agent', name: 'Scripted agent' }, provider: { id: 'provider', name: 'Team subscription' } }, access: { permissions: ['thread.view', 'thread.manage', 'thread.interact'] },
} as Thread;

beforeEach(() => change.mockClear());
afterEach(async () => { cleanup(); await setLocale('en'); });

it('offers initial setup when an imported thread has no local binding', async () => {
  show(<ComposerControls thread={{ ...thread, binding: null }} attachment={null} />);
  await userEvent.click(screen.getByRole('button', { name: /Agent: Choose an agent/ }));
  expect(screen.getByRole('dialog', { name: 'Set up thread' })).toBeVisible();
  expect(change).not.toHaveBeenCalled();
});

it('reads +, agent, model and effort, approval mode, context and Send in that order, with the sandbox inside the approval picker', () => {
  show(<ComposerControls thread={thread} attachment={<button>Add to message</button>} delivery={<button>Send</button>} />);
  const controls = within(screen.getByRole('group', { name: 'Message controls' }));
  expect(controls.getAllByRole('button').map((button) => button.getAttribute('aria-label') ?? button.textContent)).toEqual([
    'Add to message', 'Agent: Scripted agent', 'Model and effort', 'Approval mode', 'Context breakdown', 'Send',
  ]);
  const agent = controls.getByRole('button', { name: 'Agent: Scripted agent' });
  expect(agent).toHaveTextContent('Scripted agent');
  // Neither the provider nor a second picker for the sandbox stands in the row.
  expect(controls.queryByRole('button', { name: /Provider/ })).not.toBeInTheDocument();
  expect(document.querySelector('[data-composer-control="sandbox"]')).toBeNull();
});

it.each([false, true])('is one agent control and no provider control at the same widths (narrow: %s)', (narrow) => {
  if (thread.binding === null) throw new Error('The fixture has an agent binding');
  const provider = { ...providerChoice('provider').provider, name: 'Team subscription' };
  const bound = { ...thread, binding: { ...thread.binding, provider } };
  const view = show(<ViewCompactContext.Provider value={narrow}><ComposerControls thread={bound} attachment={<button>Add to message</button>} delivery={<button>Send</button>} /></ViewCompactContext.Provider>);
  expect(screen.getByRole('button', { name: 'Agent: Scripted agent' })).toBeVisible();
  expect(screen.queryByText('Team subscription')).not.toBeInTheDocument();
  expect(view.container.querySelector('[data-composer-controls]')).toHaveClass('flex-wrap');
  expect(view.container.querySelectorAll('p')).toHaveLength(0);
});

it.each(['en', 'zh-CN'] as const)('keeps the agent control closed for a driver without Manage, and says who may change it in %s', async (locale) => {
  await setLocale(locale);
  if (thread.binding === null) throw new Error('The fixture has an agent binding');
  const provider = { ...providerChoice('provider').provider, name: 'Team subscription' };
  show(<ComposerControls thread={{ ...thread, binding: { ...thread.binding, provider }, access: { ...thread.access, permissions: ['thread.view', 'thread.interact'] } }} attachment={null} />);
  const binding = screen.getByRole('button', { name: i18n.t('composer.chip.value', { what: i18n.t('composer.row.agentName'), value: 'Scripted agent' }) });
  expect(binding).toBeDisabled();
  expect(binding).toHaveAttribute('aria-description', i18n.t('composer.row.bindingReadOnly', { who: thread.owner.display_name }));
  await userEvent.click(binding); expect(change).not.toHaveBeenCalled();
});

it.each(['archived', 'side'] as const)('names the disabled agent reason for an %s thread', async (kind) => {
  const row = kind === 'archived' ? { ...thread, archived: true } : { ...thread, side_of: { id: 'parent', kind: 'thread' as const, name: 'Parent', redacted: false } };
  show(<ComposerControls thread={row} attachment={null} />);
  const binding = screen.getByRole('button', { name: 'Agent: Scripted agent' });
  expect(binding).toBeDisabled();
  expect(binding).toHaveAttribute('aria-description', i18n.t(kind === 'archived' ? 'composer.row.archived' : 'composer.row.sideBinding'));
  await userEvent.click(binding); expect(change).not.toHaveBeenCalled();
});

it.each([false, true])('gives a viewer only the notice, even with a separately held Manage permission (direct: %s)', (direct) => {
  show(direct ? <ReadOnlyComposerControls /> : <ComposerControls thread={thread} readOnly attachment={<button>Add to message</button>} delivery={<button>Stop</button>} />);
  expect(screen.getByRole('status')).toHaveTextContent('You can watch this thread. The access needed to work with it is listed above.');
  expect(screen.queryByRole('group', { name: 'Message controls' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
});


it.each(['en', 'zh-CN'] as const)('shows a thread that is only looked at what it runs with, in the words of the box, and its context figure (%s)', async (locale) => {
  await setLocale(locale);
  const provider = { ...providerChoice('provider').provider, name: 'Team subscription' };
  const looked = { ...thread, model: 'alpha-routing', model_display: 'Alpha', effort: 'high', binding: { agent: { name: 'Scripted agent' }, provider } } as unknown as Thread;
  show(<ThreadRunsLine thread={looked} />);
  const line = screen.getByRole('group', { name: i18n.t('composer.row.runsWith') });
  for (const words of ['Scripted agent', 'Alpha', i18n.t('choiceCatalog.effort.high.name')]) expect(within(line).getByText(words)).toBeVisible();
  // The provider is the model's, and is told when the model is pointed at.
  expect(line.querySelector('[data-readonly-chip="model"]')).toHaveAttribute('title', expect.stringContaining('Team subscription'));
  // Only the context figure can be opened; the chips are plain words.
  expect(within(line).getAllByRole('button').map((button) => button.textContent)).toEqual(['Context breakdown']);
  expect(change).not.toHaveBeenCalled();
});

it('shows nothing for a thread that has no binding to run with', () => {
  show(<ThreadRunsLine thread={{ ...thread, binding: null }} />);
  expect(screen.queryByRole('group', { name: i18n.t('composer.row.runsWith') })).not.toBeInTheDocument();
});
