/** Message controls retain their reading order; watch-only readers get a notice without driving controls. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import { ComposerControls, ReadOnlyComposerControls } from '@/features/composer/Controls';
import type { Thread } from '@/generated/api-types';
import { show } from './support';

const change = vi.hoisted(() => vi.fn());
vi.mock('@/features/change/target', () => ({ openChange: change }));
vi.mock('@/features/start/SetUpThread', () => ({ SetUpThread: () => <div role="dialog" aria-label="Set up thread" /> }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Model and effort</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Approval mode</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context breakdown</button> }));

const thread = { id: 'thread-1', name: 'Repair login', archived: false, state: 'idle', side_of: null, lock: null,
  owner: { display_name: 'Ada' }, binding: { agent: { name: 'Scripted agent' } }, access: { permissions: ['thread.view', 'thread.manage', 'thread.interact'] },
} as Thread;

beforeEach(() => change.mockClear());

it('offers initial setup when an imported thread has no local binding', async () => {
  show(<ComposerControls thread={{ ...thread, binding: null }} attachment={null} />);
  await userEvent.click(screen.getByRole('button', { name: /Agent: Choose an agent/ }));
  expect(screen.getByRole('dialog', { name: 'Set up thread' })).toBeVisible();
  expect(change).not.toHaveBeenCalled();
});

it('reads +, agent, model and effort, approval mode, context and Send in that order', async () => {
  show(<ComposerControls thread={thread} attachment={<button>Add to message</button>} delivery={<button>Send</button>} />);
  const controls = within(screen.getByRole('group', { name: 'Message controls' }));
  expect(controls.getAllByRole('button').map((button) => button.getAttribute('aria-label') ?? button.textContent)).toEqual([
    'Add to message', 'Agent: Scripted agent. Change binding', 'Model and effort', 'Approval mode', 'Context breakdown', 'Send',
  ]);
  const agent = controls.getByRole('button', { name: 'Agent: Scripted agent. Change binding' });
  expect(agent).toHaveTextContent('Scripted agent');
  expect(within(agent).queryByText('Agent', { exact: true })).not.toBeInTheDocument();
  expect(agent.querySelector('span')).not.toHaveClass('hidden');
  await userEvent.click(controls.getByRole('button', { name: 'Agent: Scripted agent. Change binding' }));
  expect(change).toHaveBeenCalledExactlyOnceWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
});

it.each([false, true])('gives a viewer only the notice, even with a separately held Manage permission (direct: %s)', (direct) => {
  show(direct ? <ReadOnlyComposerControls /> : <ComposerControls thread={thread} readOnly attachment={<button>Add to message</button>} delivery={<button>Stop</button>} />);
  expect(screen.getByRole('status')).toHaveTextContent('You can watch this thread. The access needed to work with it is listed above.');
  expect(screen.queryByRole('group', { name: 'Message controls' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByRole('textbox')).not.toBeInTheDocument();
  expect(change).not.toHaveBeenCalled();
});
