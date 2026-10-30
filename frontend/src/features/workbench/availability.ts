/** Retained views own their empty/refused states. The focused page never removes another view. */
import { useWorkbench } from '@/features/workbench/store';
export function useVisibleLayout() { return useWorkbench((state) => state.layout); }
