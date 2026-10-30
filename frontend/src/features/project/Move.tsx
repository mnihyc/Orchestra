/** Move to…: a project taken to another org of its tree, with who gains and who loses what said first.
 *
 * Move to…: a project taken to another org of its tree, with who gains and who loses what said first.
 * The dialog is the one an org that lies in another is moved by; a project differs only in that no
 * address changes with it.
 */
import { MoveDialog, type MoveDialogProps } from '@/features/org/Move';

export type ProjectMoveProps = MoveDialogProps;

/** The dialog that moves a project. */
export function ProjectMove(props: ProjectMoveProps) {
  return <MoveDialog {...props} />;
}
