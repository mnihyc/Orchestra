/** A conversation shown in place of the main one (a sub-agent's) opens at its end and follows what it adds there.
 *
 * It opens at its latest step, the way a conversation opens; while the reader stays at the end the view moves with each
 * step that arrives; scrolling up stops that, and coming back to the end starts it again.
 */
import { useEffect } from 'react';

/** How near its end the view has to be to go on following, in pixels. */
const AT_END_SLACK_PX = 24;

/** Puts a view at its end. */
function toEndOf(view: HTMLElement): void {
  view.scrollTop = view.scrollHeight;
}

export function useFollowEnd(viewport: HTMLElement | null, content: HTMLElement | null, key: string | null): void {
  useEffect(() => {
    if (viewport === null || content === null || key === null) return;
    let following = true;
    const toEnd = () => { toEndOf(viewport); };
    const onScroll = () => { following = viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight < AT_END_SLACK_PX; };
    toEnd();
    viewport.addEventListener('scroll', onScroll, { passive: true });
    if (typeof ResizeObserver === 'undefined') return () => viewport.removeEventListener('scroll', onScroll);
    const watch = new ResizeObserver(() => { if (following) toEnd(); });
    watch.observe(content);
    return () => { watch.disconnect(); viewport.removeEventListener('scroll', onScroll); };
  }, [viewport, content, key]);
}
