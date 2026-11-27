/** A place action stays available until the workbench's dialog host displays it. */
import { create } from 'zustand';
import type { Shortcut } from '@/api/shortcuts';
import type { DocPlace, PlaceRef } from '@/features/navigation/places';
import type { ShortcutTarget } from '@/features/navigation/shortcuts';

/** What can be asked for: each is a dialog the host draws. */
export type NodeDialog =
  | { type: 'new-org' }
  | { type: 'join' }
  | { type: 'new-department'; parent: PlaceRef }
  | { type: 'new-project'; parent: PlaceRef }
  | { type: 'new-folder'; parent: PlaceRef }
  | { type: 'new-topic'; parent: PlaceRef }
  | { type: 'new-thread'; parent: PlaceRef }
  | { type: 'new-document'; parent: PlaceRef }
  | { type: 'rename'; node: PlaceRef }
  | { type: 'archive'; node: PlaceRef }
  | { type: 'delete'; node: PlaceRef }
  | { type: 'move'; node: PlaceRef; to?: string }
  | { type: 'deleted'; node: PlaceRef }
  | { type: 'colour'; node: PlaceRef }
  | { type: 'rename-document'; doc: DocPlace }
  | { type: 'move-document'; doc: DocPlace }
  | { type: 'delete-document'; doc: DocPlace }
  | { type: 'add-shortcut'; target: ShortcutTarget }
  | { type: 'move-shortcut'; shortcut: Shortcut; name: string }
  | { type: 'error'; title: string; error: unknown };

/** How long a dialog that was closed stays drawn, for its exit: the slow step of the motion tokens. */
const EXIT_MS = 200;

interface DialogState {
  current: NodeDialog | null;
  open: boolean;
  show: (dialog: NodeDialog) => void;
  hide: () => void;
}

export const useDialogState = create<DialogState>((set, get) => ({
  current: null,
  open: false,
  show: (current) => set({ current, open: true }),
  hide: () => {
    set({ open: false });
    // The dialog leaves with its exit, and is forgotten once it has.
    setTimeout(() => {
      if (!get().open) set({ current: null });
    }, EXIT_MS);
  },
}));

/** Opens one of the dialogs. */
export function openNodeDialog(dialog: NodeDialog): void {
  useDialogState.getState().show(dialog);
}
