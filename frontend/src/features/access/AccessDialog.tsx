/** The page behind Share in its short form: the Owners line and the three lines; adding people, groups and roles, with the optional message that tells them, for one thing or for several selected at once; what goes with it; who else reaches it; and the tabs of a place.
 *
 * Share is a shortcut to the permission page of a thing and has nothing of its own: it opens the page
 * with the field that adds people in focus, so whatever was shared is found, changed and taken back
 * where every other access is. Any screen opens it with `openAccess`, and the one host of the frame
 * draws it. The dialog's own code (`AccessDialogBody.tsx`) is fetched when the first page is asked for. A place has three more tabs beside its own page: People, Defaults and Agents; the rest
 * have the page and Agents. "By area…" turns the short form into the full one, in the same dialog.
 */
import { create } from 'zustand';

import type { AccessTarget } from '@/api/access';
import { lazyPart } from '@/features/files/lazy';
import type { AccessDialogProps } from './AccessDialogBody';

export type AccessTab = 'page' | 'people' | 'defaults' | 'agents';

/** How long a dialog that was closed stays drawn, for its exit: the slow step of the motion tokens. */
export const EXIT_MS = 200;

/** The dialog itself is fetched when a page is first asked for: whoever opens one needs only `openAccess`, not the dialog's code. */
const AccessDialog = lazyPart<AccessDialogProps>(() => import('./AccessDialogBody').then((module) => ({ default: module.AccessDialog })));
const ShareSeveralDialog = lazyPart<{ targets: readonly AccessTarget[]; open: boolean; onClose: () => void }>(() => import('./Several').then((module) => ({ default: module.ShareSeveralDialog })));

interface HostState {
  target: AccessTarget | null;
  /** Several things selected together, for the form that names people on each of them. */
  many: readonly AccessTarget[] | null;
  open: boolean;
  tab: AccessTab;
  /** Opened by Share: the field that adds people has the keyboard. */
  share: boolean;
  permission: string | null;
  show: (target: AccessTarget, tab: AccessTab, share: boolean, permission: string | null) => void;
  showMany: (targets: readonly AccessTarget[]) => void;
  hide: () => void;
}

const useHost = create<HostState>((set, get) => ({
  target: null,
  many: null,
  open: false,
  tab: 'page',
  share: false,
  permission: null,
  show: (target, tab, share, permission) => set({ target, many: null, open: true, tab, share, permission }),
  showMany: (targets) => set({ target: null, many: targets, open: true }),
  hide: () => {
    set({ open: false });
    // The dialog leaves with its exit, and is forgotten once it has.
    setTimeout(() => {
      if (!get().open) set({ target: null, many: null });
    }, EXIT_MS);
  },
}));

/**
 * Opens the permission page of a thing: Share, "Access…", "Who can use it". `tab` begins it on one of
 * a place's tabs; `share` gives the field that adds people the keyboard, as Share does.
 */
export function openAccess(target: AccessTarget, options: { tab?: AccessTab; share?: boolean; permission?: string } = {}): void {
  useHost.getState().show(target, options.tab ?? 'page', options.share ?? true, options.permission ?? null);
}

/** Opens Share for several things selected together: the people named are named on each page they may give on, and the pages skipped are listed with who can change them. */
export function openAccessToMany(targets: readonly AccessTarget[]): void {
  const only = targets[0];
  if (targets.length === 1 && only !== undefined) openAccess(only);
  else if (targets.length > 1) useHost.getState().showMany(targets);
}

/** Draws the page that was asked for. Stands once in the frame of the application. */
export function AccessHost() {
  const target = useHost((state) => state.target);
  const many = useHost((state) => state.many);
  const open = useHost((state) => state.open);
  const tab = useHost((state) => state.tab);
  const share = useHost((state) => state.share);
  const permission = useHost((state) => state.permission);
  const hide = useHost((state) => state.hide);
  if (many !== null) return <ShareSeveralDialog key={many.map((entry) => entry.id).join(',')} targets={many} open={open} onClose={hide} />;
  if (target === null) return null;
  return <AccessDialog key={`${target.kind}:${target.id}:${permission ?? ''}`} target={target} open={open} onClose={hide} initialTab={tab} share={share} initialPermission={permission} />;
}
