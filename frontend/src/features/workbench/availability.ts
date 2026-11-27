/** Retained views own their empty/refused states. The focused page never removes another view. */
import { useWorkbench } from '@/features/workbench/store';
import { useShallow } from 'zustand/react/shallow';
export function useVisibleLayout() { return useWorkbench((state) => state.layout); }

export function useWorkbenchBars() {
  return useWorkbench(useShallow(({ layout }) => ({ left: layout.left, right: layout.right, bottom: layout.bottom })));
}
