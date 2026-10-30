/** Compatibility for layout menus; the rail is the single entry to the guide. */
import type { ChoiceHelp } from '@/ui/Choice';

export function useWorkbenchChoiceHelp(): (name: string) => ChoiceHelp | undefined {
  return () => undefined;
}
