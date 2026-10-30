/** A machine keeps its existing name link wherever its recorded location is shown. */
import { Link } from '@tanstack/react-router';
import type { Machine } from '@/generated/api-types';

export function MachineLink({ machine }: { machine: Pick<Machine, 'id' | 'name'> }) {
  return <Link to="/resources/machines/$id" params={{id:machine.id}} className="truncate font-medium text-text hover:underline" title={machine.name}>{machine.name}</Link>;
}
