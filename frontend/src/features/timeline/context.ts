/** What every item of a conversation is told about the conversation it stands in: the thread, how the reader likes the work shown, and what they may do.
 *
 * An item draws itself from its own row and from this, and from nothing else it has to ask for: so
 * that a conversation of thousands of items asks the same few questions once.
 */
import { createContext, useContext } from 'react';

import type { WorkDetail } from '@/features/timeline/rows';
import type { DiffLayout } from '@/ui/DiffView';

/** What picking items does to an item: whether the conversation is being picked from, whether this one is picked, and how to pick it. */
export interface ItemSelection {
  active: boolean;
  has: (id: string) => boolean;
  /** Picks or lets go of one; with `range`, of everything between the last one touched and this. */
  toggle: (id: string, range: boolean) => void;
  /** Begins picking, with this one picked. */
  start: (id: string) => void;
  /** Picks these too, beginning to pick where nothing was: "Select this turn". */
  pick: (ids: readonly string[]) => void;
}

export const NOT_PICKING: ItemSelection = { active: false, has: () => false, toggle: () => undefined, start: () => undefined, pick: () => undefined };

export interface PromptAnchor {
  inputId: string;
  itemId: string | null;
  groupId: string;
  focus?: boolean;
}
export type SelectPromptVersion = (continuationId: string | null, anchor?: PromptAnchor) => void;
export interface PromptFocus { groupId: string; request: number }

export interface TimelineContext {
  /** Stored inspection uses an isolated renderer key and this actual id only for authorized stored reads. */
  inspection?: { threadId: string; continuationId?: string } | undefined;
  /** A child stream uses the same displays with tighter spacing and no composer of its own. */
  compact?: boolean;
  /** Browsing a retained continuation keeps history links in the route without selecting execution. */
  onVersionSelect?: SelectPromptVersion | undefined;
  readOnly?: boolean;
  /** Ancestor streams prevent malformed recursive child references from rendering forever. */
  childStreams?: readonly string[];
  /** Thread ids already opened in ancestor lanes prevent a malformed recursive aside. */
  laneThreads?: readonly string[];
  threadId: string;
  /** The thread's name, for what is said of other places. */
  threadName: string;
  detail: WorkDetail;
  /** Turns whose work is shown step by step in Steps. */
  recent: ReadonlySet<string>;
  layout: DiffLayout;
  setLayout: (layout: DiffLayout) => void;
  meId: string | null;
  /** The reader holds `thread.interact`. */
  canInteract: boolean;
  /** The reader holds `thread.manage`. */
  canManage: boolean;
  /** The live connection is up: what depends on current state (answering what the agent asks) waits for it. */
  live: boolean;
  /** The items a link named, lit for a moment. */
  lit: ReadonlySet<string> | null;
  /** The items being picked, to forward, quote or discuss. */
  selection: ItemSelection;
}

const NOWHERE: TimelineContext = {
  threadId: '',
  threadName: '',
  detail: 'steps',
  recent: new Set(),
  layout: 'one',
  setLayout: () => undefined,
  meId: null,
  canInteract: false,
  canManage: false,
  live: false,
  lit: null,
  selection: NOT_PICKING,
};

export const TimelineContextValue = createContext<TimelineContext>(NOWHERE);

export function useTimelineContext(): TimelineContext {
  return useContext(TimelineContextValue);
}
