/** Message controls retain their reading order; watch-only readers get a notice without driving controls. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { ComposerControls, ReadOnlyComposerControls } from '@/features/composer/Controls';
import { i18n, setLocale } from '@/i18n';
import { ViewCompactContext } from '@/features/workbench/Narrow';
import { provider as providerChoice } from '../unit/start-fixtures';
import type { Thread } from '@/generated/api-types';
import { show } from './support';

const change = vi.hoisted(() => vi.fn());
vi.mock('@/features/change/target', () => ({ openChange: change }));
vi.mock('@/features/start/SetUpThread', () => ({ SetUpThread: () => <div role="dialog" aria-label="Set up thread" /> }));
vi.mock('@/features/composer/ModelEffort', () => ({ ModelEffort: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Model and effort</button> }));
vi.mock('@/features/composer/ApprovalMode', () => ({ ApprovalModePicker: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Approval mode</button>, ConfinementPicker: ({ readOnly }: { readOnly?: boolean }) => <button disabled={readOnly}>Sandbox</button> }));
vi.mock('@/features/usage/context/ContextRing', () => ({ ContextRing: () => <button>Context breakdown</button> }));

const thread = { id: 'thread-1', name: 'Repair login', archived: false, state: 'idle', side_of: null, lock: null,
  owner: { display_name: 'Ada' }, binding: { agent: { name: 'Scripted agent' } }, access: { permissions: ['thread.view', 'thread.manage', 'thread.interact'] },
} as Thread;

beforeEach(() => change.mockClear());
afterEach(async () => { cleanup(); await setLocale('en'); });

it('offers initial setup when an imported thread has no local binding', async () => {
  show(<ComposerControls thread={{ ...thread, binding: null }} attachment={null} />);
  await userEvent.click(screen.getByRole('button', { name: /Agent: Choose an agent/ }));
  expect(screen.getByRole('dialog', { name: 'Set up thread' })).toBeVisible();
  expect(change).not.toHaveBeenCalled();
});

it('reads +, agent, model and effort, approval mode, sandbox, context and Send in that order', async () => {
  show(<ComposerControls thread={thread} attachment={<button>Add to message</button>} delivery={<button>Send</button>} />);
  const controls = within(screen.getByRole('group', { name: 'Message controls' }));
  expect(controls.getAllByRole('button').map((button) => button.getAttribute('aria-label') ?? button.textContent)).toEqual([
    'Add to message', 'Agent: Scripted agent. Change binding', 'Model and effort', 'Approval mode', 'Sandbox', 'Context breakdown', 'Send',
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


for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) {
  it(`names the actual provider compactly and uses the same binding plan at ${width}px in ${locale}`, async () => {
    await setLocale(locale);
    if (thread.binding === null) throw new Error('The fixture has an agent binding');
    const provider = { ...providerChoice('provider').provider, name: 'Team subscription' };
    const bound = { ...thread, binding: { ...thread.binding, provider } };
    const view = show(<div style={{ width }}><ViewCompactContext.Provider value={width === 400}><ComposerControls thread={bound} attachment={<button>Add to message</button>} delivery={<button>Send</button>} /></ViewCompactContext.Provider></div>);
    const binding = screen.getByRole('button', { name: i18n.t('composer.row.agent', { name: 'Scripted agent' }) });
    expect(within(binding).getByText(provider.name)).toBeVisible();
    expect(binding).toHaveAccessibleDescription(i18n.t('composer.row.provider', { name: provider.name }));
    expect(within(binding).queryAllByRole('button')).toHaveLength(0);
    expect(view.container.querySelector('[data-composer-controls]')).toHaveClass('flex-wrap');
    expect(view.container.querySelectorAll('p')).toHaveLength(0);
    await userEvent.click(binding);
    expect(change).toHaveBeenCalledWith({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
  });
}

it.each(['en', 'zh-CN'] as const)('keeps binding changes guarded for a driver without Manage in %s', async (locale) => {
  await setLocale(locale);
  if (thread.binding === null) throw new Error('The fixture has an agent binding');
  const provider = { ...providerChoice('provider').provider, name: 'Team subscription' };
  show(<ComposerControls thread={{ ...thread, binding: { ...thread.binding, provider }, access: { ...thread.access, permissions: ['thread.view', 'thread.interact'] } }} attachment={null} />);
  const binding = screen.getByRole('button', { name: i18n.t('composer.row.agent', { name: 'Scripted agent' }) });
  expect(binding).toHaveAttribute('aria-disabled', 'true');
  expect(binding).toHaveAccessibleDescription(`${i18n.t('composer.row.provider', { name: provider.name })} · ${i18n.t('composer.row.bindingReadOnly', { who: thread.owner.display_name })}`);
  await userEvent.click(binding); expect(change).not.toHaveBeenCalled();
});

it.each(['archived', 'side'] as const)('names the disabled binding reason for an %s thread', async (kind) => {
  const row = kind === 'archived' ? { ...thread, archived: true } : { ...thread, side_of: { id: 'parent', kind: 'thread' as const, name: 'Parent', redacted: false } };
  show(<ComposerControls thread={row} attachment={null} />);
  const binding = screen.getByRole('button', { name: i18n.t('composer.row.agent', { name: 'Scripted agent' }) });
  expect(binding).toHaveAttribute('aria-disabled', 'true');
  expect(binding).toHaveAccessibleDescription(i18n.t(kind === 'archived' ? 'composer.row.archived' : 'composer.row.sideBinding'));
  await userEvent.click(binding); expect(change).not.toHaveBeenCalled();
});
