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
import { create } from 'zustand';

import type { ProviderRef, UsageCap } from '@/generated/api-types';
import { CapDialog } from '@/features/usage/CapDialog';

// ---------------------------------------------------------------- opening it from anywhere

/** Where a cap is made, and what it covers to begin with. */
export interface CapRequest {
  /** A place the cap is set on. */
  node?: { id: string };
  /** A provider the cap is set on. */
  provider?: Pick<ProviderRef, 'id'> & Partial<Pick<ProviderRef, 'name'>>;
  /** Models by name, families (`gpt-6*`) and groups (`@frontier`) it covers to begin with; none is every model. */
  models?: readonly string[];
  /** The speeds, by id, it covers to begin with; none is any speed. */
  tiers?: readonly string[];
  /** A cap to change instead of one to make. */
  edit?: UsageCap;
}

interface Host {
  request: CapRequest | null;
  /** Counts the times it was opened, so that each opening begins from a form of its own. */
  opened: number;
  open: boolean;
  show: (request: CapRequest) => void;
  hide: () => void;
}

/** How long a dialog that was closed stays drawn, for its exit: the slow step of the motion tokens. */
const EXIT_MS = 200;

const useHost = create<Host>((set, get) => ({
  request: null,
  opened: 0,
  open: false,
  show: (request) => set((state) => ({ request, open: true, opened: state.opened + 1 })),
  hide: () => {
    set({ open: false });
    setTimeout(() => {
      if (!get().open) set({ request: null });
    }, EXIT_MS);
  },
}));

/** Opens the form that makes a cap, or changes one, with a provider or a place and what it covers filled in. */
export function openCap(request: CapRequest): void {
  useHost.getState().show(request);
}

/** Draws the form that was asked for. Stands once in the frame of the application. */
export function CapHost() {
  const request = useHost((state) => state.request);
  const opened = useHost((state) => state.opened);
  const open = useHost((state) => state.open);
  const hide = useHost((state) => state.hide);
  if (request === null) return null;
  return <CapDialog key={opened} request={request} open={open} onClose={hide} />;
}
