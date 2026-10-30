/** What can be changed and how, and the one entry point every screen calls: `openChange`.
 *
 * A fork or a switch of a thread, several threads switched together, an environment brought up to date,
 * recreated or moved, a provider's runtime updated, and what is idle on a host or at a place stopped to make
 * room: one wizard with the same four views (the plan, the confirmation, the job's progress, the end), opened
 * from wherever the thing is shown. A screen names the thing and what is to be done to it; the one host that
 * stands in the frame of the application draws the wizard. The job is the server's, so a wizard that was hidden
 * or closed is opened again from a banner, a chip or a card by naming the job.
 */
import { create } from 'zustand';

/** What is done to the thing named. */
export type ChangeKind =
  /** Another provider, agent profile or environment for a thread, which goes on as it is. */
  | 'switch'
  /** A second thread made out of a thread, or out of a side thread kept. */
  | 'fork'
  /** A unit brought up to what it would be made from now. */
  | 'update'
  /** A new unit with nothing new in it. */
  | 'recreate'
  /** An environment, with everything in it, on another host. */
  | 'move'
  /** What is idle stopped to free room. */
  | 'recycle'
  /** Several environments recovered independently. */
  | 'recover'
  /** The view of a job that is under way or over, asked for by its number. */
  | 'progress';

/** What a screen names: the thing the change is about. */
export type ChangeTarget =
  | { kind: 'thread'; id: string; name?: string }
  /** Several threads, listed by name. `from` is where they were gathered, for the title that says so. */
  | { kind: 'threads'; threads: { id: string; name: string }[]; from?: { kind: 'topic' | 'provider' | 'environment'; name: string } }
  | { kind: 'topic'; id: string; name?: string }
  | { kind: 'provider'; id: string; name?: string }
  | { kind: 'agent'; id: string; name?: string }
  | { kind: 'model'; providerId: string; id: string; name?: string }
  | { kind: 'environment'; id: string; name?: string }
  /** Several environments of one place, listed. */
  | { kind: 'environments'; ids: string[]; node?: string }
  | { kind: 'machine'; id: string; name?: string }
  | { kind: 'host'; id: string; name?: string }
  /** A place with environments in it: an org, a project or a topic. */
  | { kind: 'node'; id: string; name?: string };

/** How a change begins, beyond what it is. */
export interface ChangeOptions {
  kind: ChangeKind;
  /** For a fork, the turn whose end it takes the history up to: "Fork from here". */
  toTurn?: number;
  /** For a switch, what the person came for: another provider, or another environment or directory. */
  focus?: 'provider' | 'environment';
  /** For the progress of a job, or of a batch, that was started already. */
  job?: string;
  batch?: string;
  /** For a fork made from a message of a person: what it said, to be put in the new thread's box. */
  say?: string;
}

/** What the host holds: the change asked for, until it is closed. */
export interface Asked {
  target: ChangeTarget;
  options: ChangeOptions;
  /** A new number each time, so that asking again for the same thing opens it anew. */
  asked: number;
}

interface HostState {
  current: Asked | null;
  open: boolean;
  show: (target: ChangeTarget, options: ChangeOptions) => void;
  hide: () => void;
}

/** How long a wizard that was closed stays drawn, for its exit: the slow step of the motion tokens. */
const EXIT_MS = 200;

let counter = 0;

export const useChangeHost = create<HostState>((set, get) => ({
  current: null,
  open: false,
  show: (target, options) => {
    counter += 1;
    set({ current: { target, options, asked: counter }, open: true });
  },
  hide: () => {
    set({ open: false });
    // The wizard leaves with its exit, and is forgotten once it has.
    setTimeout(() => {
      if (!get().open) set({ current: null });
    }, EXIT_MS);
  },
}));

/** Opens the wizard of a change: "Fork…", "Switch…", "Update…", "Move…", "Recycle idle…", or the progress of a job. */
export function openChange(target: ChangeTarget, options: ChangeOptions): void {
  useChangeHost.getState().show(target, options);
}

/** The three kinds of work that lock what they change, in the words a screen asks for them. */
export function locks(kind: ChangeKind): boolean {
  return kind === 'switch' || kind === 'update' || kind === 'recreate' || kind === 'move';
}
