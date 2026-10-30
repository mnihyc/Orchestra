/** The standard access program and the absence of a program are distinct slash choices. */
import { renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { useModelCommands } from '@/features/composer/SlashMenu';
import type { SpeedOrAccess } from '@/features/composer/modelState';
import type { Thread } from '@/generated/api-types';

const fake = vi.hoisted(() => ({ choose: vi.fn() }));
const state: SpeedOrAccess = { kind: 'access', key: 'access', title: 'Access', description: '', apply: 'next_turn', writable: true, lockedBy: null, place: null, from: 'start', stale: false, start: null, chosen: null, force: null, open: [{ id: 'standard', label: 'Standard' }, { id: 'daybreakBlue', label: 'Daybreak Blue' }] };
vi.mock('@/features/composer/runsWith', () => ({ useRunsWith: () => ({ states: [state], choose: fake.choose }) }));

it('chooses the offered Standard program instead of clearing the program', () => {
  const thread = { binding: {}, access: { permissions: ['thread.manage'] }, owner: { display_name: 'Ada' } } as unknown as Thread;
  const { result } = renderHook(() => useModelCommands(thread, () => undefined));
  const command = result.current.find((one) => one.name === 'access');
  if (command === undefined) throw new Error('The access command was not offered');
  expect(command.run('standard')).toBeNull();
  expect(fake.choose).toHaveBeenLastCalledWith(state, 'standard');
  expect(command.run('daybreak blue')).toBeNull();
  expect(fake.choose).toHaveBeenLastCalledWith(state, 'daybreakBlue');
  expect(command.run('none')).toBeNull();
  expect(fake.choose).toHaveBeenLastCalledWith(state, null);
});
