/** Opening the help: beside what the reader is doing, or as a screen of its own where there is no room beside.
 *
 * On a screen with areas the help is a view in the right bar, and where the right bar lies over the
 * main area it is opened over it. On a small screen there are no areas, and the help is its own screen,
 * which has an address like any other (`/help/<article>`).
 */
import { useRouter } from '@tanstack/react-router';

import { showArticle, useHelpState } from '@/features/help/state';
import { open } from '@/features/workbench/layout';
import { useShape } from '@/features/workbench/Narrow';
import { useWorkbench } from '@/features/workbench/store';
import { currentPrompt } from '@/ui/PromptScope';

/** Opens the help, at an article and a heading in it, or at its first page. */
export function useHelpOpener(): (article?: string, heading?: string) => void {
  const router = useRouter();
  const shape = useShape();
  const change = useWorkbench((state) => state.change);
  const setOverlay = useWorkbench((state) => state.setOverlay);
  return (article, heading) => {
    useHelpState.setState({ guideContext: null });
    const location = router.state.location;
    if (!location.pathname.startsWith('/help')) useHelpState.setState({ origin: location.pathname });
    const dialog = currentPrompt();
    if (dialog !== null && document.querySelector('[data-help-work]') !== null) {
      showArticle(article ?? null, heading ?? null);
      // The Shell pauses its work subtree, preserving drafts and releasing every focus trap.
      useHelpState.setState((state) => ({ overlay: true, readingRequest: state.readingRequest + 1 }));
      return;
    }
    if (location.pathname.startsWith('/help')) {
      if (article !== undefined) router.history.push(`/help/${encodeURIComponent(article)}${heading === undefined ? '' : `#${encodeURIComponent(heading)}`}`);
      return;
    }
    if (shape === 'small') {
      router.history.push(article === undefined ? '/help' : `/help/${encodeURIComponent(article)}${heading === undefined ? '' : `#${encodeURIComponent(heading)}`}`);
      return;
    }
    showArticle(article ?? null, heading ?? null);
    change((layout) => open(layout, 'help', 'right'));
    if (shape !== 'wide') setOverlay('right');
  };
}
