/** One article of the user guide as it ships with the build: its headings, links that stay in the view, links that open a screen, and its address.
 *
 * The Markdown of the article is drawn as every other Markdown is, into elements and never into markup. What
 * this adds is what a guide needs: each heading has an anchor (`/help/<article>#<heading>`), a link to
 * another article (`other.md`) stays in the help, and a link to a screen of the application opens that
 * screen. Nothing is fetched: the article is a part of the build.
 */
import { ArrowLeft } from 'lucide-react';
import { type MouseEvent, type ReactNode, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { type Article, type Guide } from '@/help';
import { slugOf } from '@/help/guide';
import { resolveArticleLink, rewriteGuideLinks } from '@/help/links';
import { Button } from '@/ui/Button';
import { Markdown } from '@/ui/Markdown';

/** Links to other files of the guide are written `name.md` or `name.md#heading`; in the application they are `/help/name`. */
export function rewriteLinks(markdown: string, prefix = '', files: Readonly<Record<string, string>> = {}): string {
  return rewriteGuideLinks(markdown, prefix, files);
}

/** Where a link of an article goes: to another article of the guide, or to a screen. */
export function helpTarget(href: string): { article: string; heading: string | null } | null {
  return href.startsWith('/help/') ? resolveArticleLink(href) : null;
}

export interface ArticleViewProps {
  article: Article;
  /** The heading to bring into view. */
  heading: string | null;
  /** A link to another article was followed. */
  onArticle: (article: string, heading: string | null) => void;
  /** A link to a screen was followed. */
  onScreen: (href: string) => void;
  /** Goes back to the list; absent where the article is a page of its own. */
  onBack?: () => void;
  /** Counts how many times the view was led to this place, so that going to the same heading again scrolls again. */
  visit?: number;
  /** Related tutorials follow the article in its scroll flow, never over its last paragraph. */
  children?: ReactNode;
}

export function ArticleView({ article, heading, onArticle, onScreen, onBack, visit = 0, children }: ArticleViewProps) {
  const { t } = useTranslation();
  const body = useRef<HTMLDivElement>(null);
  const text = useMemo(() => rewriteLinks(article.body, article.linkPrefix, article.files), [article.body, article.linkPrefix, article.files]);
  const landed = useRef<{ article: string; heading: string | null; target: string | null; visit: number; text: string } | null>(null);

  // Each heading gets its anchor, and the one that was asked for is brought into view.
  useEffect(() => {
    const root = body.current;
    if (root === null) return;
    // Saved translated links keep working when sections share an address across languages.
    const headings = [...article.headings, ...Object.values(article.translations ?? {}).flatMap((one) => one.headings)];
    const target = heading === null ? null : (headings.find((one) => one.id === heading || slugOf(one.text) === heading)?.id ?? heading);
    const previous = landed.current;
    if (previous?.article === article.id && previous.heading === heading && previous.target === target && previous.visit === visit && previous.text === text) return;
    landed.current = { article: article.id, heading, target, visit, text };
    for (const element of root.querySelectorAll<HTMLElement>('h1, h2, h3, h4')) {
      const section = article.headings.find((one) => one.level === Number(element.tagName.slice(1)) && slugOf(one.text) === slugOf(element.textContent));
      element.id = section?.id ?? slugOf(element.textContent);
    }
    if (target !== null) root.querySelector<HTMLElement>(`[id="${CSS.escape(target)}"]`)?.scrollIntoView({ block: 'start' });
    else root.scrollTop = 0;
  }, [article.id, article.headings, article.translations, text, heading, visit]);

  const follow = (event: MouseEvent<HTMLDivElement>) => {
    const link = (event.target as HTMLElement).closest('a[href]');
    if (link === null || event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
    const href = link.getAttribute('href') ?? '';
    // A schema is a static artifact of this build; let its link open the asset, not a router screen.
    if (Object.values(article.files ?? {}).includes(href)) {
      link.setAttribute('target', '_blank');
      link.setAttribute('rel', 'noopener noreferrer');
      return;
    }
    const inside = helpTarget(href);
    if (inside !== null) {
      event.preventDefault();
      onArticle(inside.article, inside.heading);
    } else if (href.startsWith('/')) {
      event.preventDefault();
      onScreen(href);
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {onBack !== undefined && (
        <div className="shrink-0 border-b border-border px-3 py-1">
          <Button action="back" size="sm" variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={onBack}>
            {t('help.articles.back')}
          </Button>
        </div>
      )}
      {/* An article is read in its authored language; Help names an English body when needed. */}
      {/* An article scrolls by itself, so that it takes the keyboard: its arrow keys and Page keys then read it. */}
      <div ref={body} role="region" aria-label={article.title} tabIndex={0} className="min-h-0 flex-1 overflow-y-auto p-4" onClick={follow}>
        <Markdown text={text} />
        {children}
      </div>
    </div>
  );
}

export type { Guide };
