/** The requested limit form, shared by its entry points and the workbench dialog host. */
import { create } from 'zustand';

import type { ProviderRef, UsageCap } from '@/generated/api-types';

// ---------------------------------------------------------------- opening it from anywhere

/** Where a cap is made, and what it covers to begin with. */
export interface CapRequest {
  /** A place the cap is set on. */
  node?: { id: string };
  /** A provider the cap is set on. */
  provider?: Pick<ProviderRef, 'id'> & Partial<Pick<ProviderRef, 'name'>>;
  /** Models by name, families (`gpt-6*`) and groups (`@frontier`) it covers to begin with; none is every model. */
  models?: readonly string[];
  services?: readonly string[];
  purposes?: readonly string[];
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

export const useCapRequest = create<Host>((set, get) => ({
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
  useCapRequest.getState().show(request);
}
