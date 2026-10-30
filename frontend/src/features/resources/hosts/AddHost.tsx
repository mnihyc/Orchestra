/** Add host opens the same explicit, HTTPS-gated enrollment with Node selected. */
import { AddMachineDialog } from '@/features/resources/machines/Enrollment';

export function AddHostDialog({onClose}:{onClose:()=>void}) {
  return <AddMachineDialog onClose={onClose} initialKind="node"/>;
}
