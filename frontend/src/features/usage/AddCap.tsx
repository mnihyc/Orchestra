/** Sentence-shaped cap form: who, how much of what (tokens, credits or turns at once), on which models or family and which agent, where; with its effects stated.
 *
 * "Everyone together may use at most 2M tokens a day on all models in project Web." is the sentence the
 * form completes, in the order the language says it: whom it covers, the amount and its unit, the period,
 * and the models, which are named, or are a family by the beginning of their names, or are the group a
 * provider's managers made of several. The visible fields include the agents and speeds it narrows to, the
 * warning, what happens at the cap and to work that cannot be counted, and a note. Whoever changes a cap
 * changes what may change (its amount, what it covers, how it holds); the rest is fixed when it is made.
 * `openCap` opens it from anywhere, with a provider or a place and what it covers filled in. Its code
 * is ready with the host; fetching a separate form chunk must not delay the first opening.
 */
import { CapDialog } from '@/features/usage/CapDialog';
import { useCapRequest } from '@/features/usage/capRequest';

/** Draws the form that was asked for. Stands once in the frame of the application. */
export function CapHost() {
  const request = useCapRequest((state) => state.request);
  const opened = useCapRequest((state) => state.opened);
  const open = useCapRequest((state) => state.open);
  const hide = useCapRequest((state) => state.hide);
  if (request === null) return null;
  return <CapDialog key={opened} request={request} open={open} onClose={hide} />;
}
