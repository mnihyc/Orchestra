/** A remembered navigation tree starts early only when that navigation will be visible. */
import { activeContainer } from '@/features/workbench/layout';
import { loadWorkspace } from '@/features/workbench/persistence';
import { viewportShape } from '@/features/workbench/viewport';
import { readViewChoice } from '@/lib/viewChoices';

export function rememberedNavigationShown(userId: string): boolean {
  // The phone navigation drawer always begins closed, independently of desktop bar choices.
  if (viewportShape() === 'small') return false;
  const kept = loadWorkspace(userId).kept;
  if (kept !== null) {
    if (kept.focusMode || !kept.layout.left.open) return false;
    const container = activeContainer(kept.layout.left);
    return container?.sections.some((section) => section.tab.view === 'navigation' && section.open && !section.hidden) === true;
  }
  return readViewChoice<boolean>(userId, 'global', 'leftOpen', true)
    && readViewChoice<string>(userId, 'global', 'leftContainer', 'navigation') === 'navigation';
}
