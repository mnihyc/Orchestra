/** The commands there are: what a screen offers to do, kept in one place so that the finder lists what can be done where the reader is.
 *
 * A command is something to do (make a topic, go to the inbox, switch the theme), with its name in
 * the reader's language, the words that find it, where it leads when all it does is lead somewhere,
 * and whether it can be used at the moment. The shell registers the ones that hold everywhere; a
 * screen registers its own while it is in view (`useRegisterCommands`), and they are gone when it goes.
 * A command registered again under the same id replaces the one before it.
 */
import { useViewInstance } from '@/features/workbench/instance';
import { useWorkbench } from '@/features/workbench/store';
import type { TFunction } from 'i18next';
import type { LucideIcon } from 'lucide-react';
import { useEffect, useSyncExternalStore } from 'react';

/** What sort of thing a command does: it names the word under it in the finder and orders the list. */
export type CommandKind = 'place' | 'create' | 'go' | 'view' | 'appearance' | 'account';

/** Whether a command can be used now: yes, not at all (it is left out), or no with the reason (it is listed switched off). */
export type Availability = true | false | { disabled: string };

/** How a command is run: where the reader asked for it to open, for those that lead somewhere. */
export type OpenHow = 'here' | 'tab' | 'beside';

export interface Command {
  /** The command's name for good: `go.inbox`, `topic.new`. */
  id: string;
  kind: CommandKind;
  /** What it is called, in the reader's language. */
  title: (t: TFunction) => string;
  /** More words that find it, in the reader's language. */
  keywords?: (t: TFunction) => readonly string[];
  /** What stands under its name in the list instead of what kind of command it is: the place it acts on, say. */
  about?: (t: TFunction) => string;
  /** Short trailing context, such as a view's container. */
  note?: (t: TFunction) => string;
  icon?: LucideIcon;
  /** The key that does the same, shown beside it. The key itself is bound elsewhere. */
  shortcut?: string;
  /** Where it leads, as a path with its search: set for a command that only goes somewhere, so that it can be opened in a new tab. */
  path?: string;
  /** Does it. `how` is where the reader asked for it to open, which only a command that leads somewhere heeds. */
  run: (how: OpenHow) => void;
  /** The command opens another list inside this finder. */
  keeps?: boolean;
  /** Absent: yes. */
  available?: () => Availability;
}

const commands = new Map<string, Command>();
const listeners = new Set<() => void>();
let snapshot: readonly Command[] = [];

function changed(): void {
  snapshot = [...commands.values()];
  for (const listener of listeners) listener();
}

/** Registers commands. What it returns takes them away again, except any that another registration replaced meanwhile. */
export function registerCommands(list: readonly Command[]): () => void {
  for (const command of list) commands.set(command.id, command);
  changed();
  return () => {
    let removed = false;
    for (const command of list) {
      if (commands.get(command.id) === command) {
        commands.delete(command.id);
        removed = true;
      }
    }
    if (removed) changed();
  };
}

/** Registers commands for as long as the calling component is shown. Give a list that stays the same until what it says changes (`useMemo`). */
export function useRegisterCommands(list: readonly Command[]): void {
  const instance = useViewInstance();
  const focused = useWorkbench((state) => state.focusedTab);
  const admitted = instance === null || focused === instance.id;
  useEffect(() => admitted ? registerCommands(list) : undefined, [list, admitted]);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Every command registered, in the order they were registered, following those that come and go. */
export function useCommands(): readonly Command[] {
  return useSyncExternalStore(subscribe, () => snapshot);
}

/** The commands registered now, for code that is not a component. */
export function commandsNow(): readonly Command[] {
  return snapshot;
}
