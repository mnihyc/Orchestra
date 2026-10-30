/** Which thread's goal editor is open: the thread's ⋯ menu, the line's Edit and the slash command all open the one dialog. */
import { create } from 'zustand';

interface GoalEditorStore {
  /** The thread whose goal is being set or edited. */
  threadId: string | null;
}

const useStore = create<GoalEditorStore>(() => ({ threadId: null }));

/** Opens the editor for a thread. */
export function openGoalEditor(threadId: string): void {
  useStore.setState({ threadId });
}

/** Closes the editor. */
export function closeGoalEditor(): void {
  useStore.setState({ threadId: null });
}

/** The thread whose editor is open, if any. */
export function useGoalEditorThread(): string | null {
  return useStore((state) => state.threadId);
}
