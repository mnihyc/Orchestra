/** A managed machine names and opens its host; a connected machine opens its own page. */
import { Link } from '@tanstack/react-router';
import type { MachineRef } from '@/generated/api-types';

export function MachineLink({ machine }: { machine: Pick<MachineRef, 'id' | 'name'> & Partial<Pick<MachineRef, 'placement' | 'host_id'>> }) {
  if (machine.placement === 'managed' && machine.host_id !== null && machine.host_id !== undefined) {
    return <Link to="/resources/hosts/$id" params={{id:machine.host_id}} className="truncate font-medium text-text hover:underline" title={machine.name}>{machine.name}</Link>;
  }
  return <Link to="/resources/machines/$id" params={{id:machine.id}} className="truncate font-medium text-text hover:underline" title={machine.name}>{machine.name}</Link>;
}
