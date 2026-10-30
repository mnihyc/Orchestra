/** What the help view shows at the moment: the article it was led to, the heading in it, and the words typed in its search.
 *
 * The help is a view among the others, which the finder, a hint's "More", the status bar and `F1` all open.
 * So that each of them can say where in the help it wants to be, that place is kept here and not in the
 * view, which may be shown, hidden and moved while the place stays.
 */
import { create } from 'zustand';
import type { SettingsTarget } from '@/api/settings';

interface HelpState {
  /** The article in front, or `null` for the first page of the help: this screen, its keys, the articles. */
  article: string | null;
  /** The heading of the article to bring into view. */
  heading: string | null;
  /** Counts the times somebody led the view to a place, so that going to the same heading twice is two journeys. */
  visit: number;
  query: string;
  /** Only a key/place, never a current value or credential; the entry reads its source under current authority. */
  settingContext: { key: string; target: SettingsTarget } | null;
  /** A guide's related setting entries retain only the originating target, never its values. */
  guideContext: { article: string; target: SettingsTarget } | null;
  /** The work screen a narrow Help page was opened from; never a setting value. */
  origin: string | null;
  /** A normal reading view temporarily replaces the work; its paused drafts survive returning. */
  overlay: boolean;
  /** A later request can bring reading back above a newer global prompt. */
  readingRequest: number;
}

export const useHelpState = create<HelpState>(() => ({ article: null, heading: null, visit: 0, query: '', settingContext: null, guideContext: null, origin: null, overlay: false, readingRequest: 0 }));

/** Leads the help view to an article, and to a heading in it. */
export function showArticle(article: string | null, heading: string | null = null): void {
  useHelpState.setState((state) => ({ article, heading, visit: state.visit + 1, guideContext: null }));
}

export function setHelpQuery(query: string): void {
  useHelpState.setState({ query });
}
