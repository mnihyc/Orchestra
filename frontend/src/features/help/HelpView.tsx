/** The help view: what there is for the screen in front, search, the articles, the tutorials, the shortcuts and what is new in this build.
 *
 * It is a view like any other: it opens beside what the reader is doing, from `F1`, the rail,
 * a guide link and a result of the finder, and it is the screen at `/help` where
 * there is no room beside. For this screen it says what the screen is for, lists the hints that are on it, and
 * names the articles that belong to it; it searches the articles by title, heading and keyword; it lists
 * every article, and the keys that are bound now. Everything in it is a part of the build: no request is
 * made, and no model is asked.
 */
import { useViewLocation, useViewNavigate } from '@/features/workbench/pageContext';
import { useMatches, useRouter } from '@tanstack/react-router';
import { BookOpen, GraduationCap, Keyboard, Lightbulb, SearchX, SlidersHorizontal, Newspaper } from 'lucide-react';
import { type ReactNode, useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { answer, helpEntries } from '@/features/finder/local';
import { ArticleView } from '@/features/help/Article';
import { GuideSettings } from '@/features/help/GuideSettings';
import { SettingEntry } from '@/features/help/SettingEntry';
import { TutorialList } from '@/features/help/Tutorial';
import { createTutorialDock } from '@/features/help/tutorialState';
import { WhatsNew } from '@/features/help/WhatsNew';
import { findSettingHelp, settingArticle, settingHelpEntries, settingKeyOf } from '@/features/help/settings';
import { hintOf } from '@/features/help/hints';
import { useMountedHints } from '@/features/help/mounted';
import { KeyList } from '@/features/help/KeyList';
import { useHelpState, setHelpQuery, showArticle } from '@/features/help/state';
import { useDismissed } from '@/features/help/Dismissed';
import { type Article, belongsTo, type Guide } from '@/help';
import { useLocalizedGuide as useGuide } from '@/help/localized';
import { articleAvailable, articleMatchesLanguage } from '@/help/guide';
import { useViewChoice } from '@/lib/viewChoices';
import type { TranslationKey } from '@/i18n';
import { EmptyState } from '@/ui/EmptyState';
import { HelpPanel } from '@/ui/HelpPanel';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Button } from '@/ui/Button';
import { PageHeader } from '@/shell/Page';
import { usePromptOpen } from '@/ui/PromptScope';
import { helpSection, type HelpSection } from '@/features/help/navigation';
import { screenAbout } from '@/features/help/screenWords';

type Part = 'screen' | HelpSection;

/** The route of the screen in view, as the table of routes writes it (`/thread/$threadId`). */
export function useScreenRoute(): string {
  const id = useMatches({ select: (matches) => matches.at(-1)?.routeId }) ?? '/';
  return id.length > 1 ? id.replace(/\/$/, '') : id;
}

/** What goes with the help: the guide, the place in it, and how the reader moves about. */
function useHelp(page: boolean) {
  const router = useRouter();
  const navigate = useViewNavigate();
  const state = useHelpState();
  const guide = useGuide();
  const open = (article: string | null, heading: string | null = null) => {
    useHelpState.setState({ guideContext: null });
    if (page) void navigate({ href: article === null ? '/help' : `/help/${encodeURIComponent(article)}${heading === null ? '' : `#${encodeURIComponent(heading)}`}` });
    else showArticle(article, heading);
  };
  return { state, guide, open, goTo: (href: string) => { useHelpState.setState({ overlay: false }); if (page) void navigate({ href }); else router.history.push(href); } };
}

export interface HelpContentProps {
  /** The help as a page of its own, whose articles have addresses, and not as a view beside another screen. */
  page?: boolean;
  /** The article the address names, on the page of an article. */
  article?: string | null;
  heading?: string | null;
}

export function HelpContent(props: HelpContentProps) {
  const prompting = usePromptOpen();
  // Hide the reading surface, keeping a prompt opened by its own setting editor alive in its portal.
  return <div className="contents" inert={prompting} style={prompting ? { visibility: 'hidden' } : undefined}><HelpDocument {...props} /></div>;
}

function HelpDocument({ page = false, article: routed = null, heading: routedHeading = null }: HelpContentProps) {
  const { t, i18n } = useTranslation();
  const { state, guide, open, goTo } = useHelp(page);
  const [part, setPart] = useViewChoice<Part>('help', 'part', 'screen');
  const route = useScreenRoute();
  const about = screenAbout(route);
  const location = useViewLocation();
  const navigate = useViewNavigate();
  const routedPart = helpSection((location.search as Record<string, unknown>).part);
  const [initialPagePart] = useState(() => helpSection(part) ?? 'articles');
  const [pageQuery, setPageQuery] = useState('');
  const [pageSettingContext] = useState(state.settingContext);
  const queryText = page ? pageQuery : state.query;
  const setQuery = page ? setPageQuery : setHelpQuery;
  const ownPage = page;
  const shownPart = ownPage ? routedPart ?? initialPagePart : helpSection(part) ?? (about === undefined ? 'articles' : 'screen');
  const selectPart = (next: Part) => {
    setPart(next);
    if (ownPage && next !== 'screen') void navigate({ href: `/help?part=${encodeURIComponent(next)}`, replace: true });
  };
  const dock = useMemo(() => createTutorialDock(), []);

  const wanted = page ? routed : state.article;
  const wantedHeading = page ? routedHeading : state.heading;
  const chosen = guide?.articles.find((candidate) => candidate.id === wanted && articleAvailable(candidate));
  const entries = useMemo(() => helpEntries(guide?.articles ?? []), [guide]);
  const query = queryText.trim();
  const allArticles = () => { setQuery(''); selectPart('articles'); if (!ownPage) open(null); };
  const found = useMemo(() => (query === '' ? [] : answer(entries, query).slice(0, 30)), [entries, query]);
  const settingDefs = settingHelpEntries(i18n);
  const settingKey = settingKeyOf(wanted);
  const settingDef = settingDefs.find((def) => def.key === settingKey);
  useEffect(() => {
    if (!ownPage || wanted === null) return;
    const actual = settingDef !== undefined ? 'settings' : chosen === undefined ? undefined : chosen.section === 'build-your-own' ? 'build' : 'articles';
    if (actual !== undefined && actual !== part) setPart(actual);
  }, [ownPage, wanted, settingDef, chosen, part, setPart]);
  const matchingSettings = query === '' ? [] : findSettingHelp(settingDefs, query);
  const allSettings = () => { setQuery(''); selectPart('settings'); if (!ownPage) open(null); };
  const settingContext = page ? pageSettingContext : state.settingContext;
  const settingTarget = settingContext?.key === settingKey ? settingContext.target : undefined;

  const search = (
    <Input
      label={t('help.search.label')}
      hideLabel
      type="search"
      value={queryText}
      onChange={(event) => setQuery(event.target.value)}
      placeholder={t('help.search.placeholder')}
      autoComplete="off"
      spellCheck={false}
    />
  );

  let covering: ReactNode;
  if (settingDef !== undefined) covering = <SettingEntry def={settingDef} target={settingTarget} onBack={allSettings} />;
  else if (guide === undefined) covering = <SkeletonRows rows={5} className="p-3" />;
  else if (chosen !== undefined) {
    covering = (
      <>
        {!articleMatchesLanguage(chosen, i18n.resolvedLanguage ?? i18n.language) && <p className="border-b border-border bg-bg-subtle px-4 py-2 text-sm text-text-muted">{t('help.articles.untranslated')}</p>}
        <ArticleView
          article={chosen}
          heading={wantedHeading}
          visit={state.visit}
          onArticle={(next, heading) => open(next, heading)}
          onScreen={goTo}
          {...(page ? {} : { onBack: allArticles })}
        ><GuideSettings article={chosen.id} onOpen={(key) => {
          const target = state.guideContext?.article === chosen.id ? state.guideContext.target : undefined;
          open(settingArticle(key));
          useHelpState.setState({ settingContext: target === undefined ? null : { key, target } });
        }} /><TutorialList article={chosen.id} /></ArticleView>
      </>
    );
  } else if (wanted !== null) covering = <EmptyState compact icon={<SearchX aria-hidden />} title={t('help.articles.missing')} action={<Button action="back" size="sm" onClick={allArticles}>{t('help.articles.back')}</Button>} />;
  else if (query !== '') {
    covering =
      found.length === 0 && matchingSettings.length === 0 ? (
        <EmptyState compact icon={<SearchX aria-hidden />} title={t('help.search.nothing', { query })} description={t('help.search.nothingAbout')} />
      ) : (
        <ul className="flex flex-col p-2">
          {matchingSettings.map((def) => <li key={`setting:${def.key}`}><button type="button" className="focus-inset flex w-full flex-col items-start rounded-md px-2 py-2 text-left hover:bg-bg-muted" onClick={() => { setQuery(''); open(settingArticle(def.key)); }}><span className="text-base text-text">{def.title}</span><span className="text-sm text-text-muted">{def.description}</span></button></li>)}
          {found.map(({ item }) => (
            <li key={`${item.article.id}#${item.heading?.id ?? ''}`}>
              <button
                type="button"
                className="focus-inset flex w-full flex-col items-start rounded-md px-2 py-2 text-left hover:bg-bg-muted"
                onClick={() => {
                  setQuery('');
                  open(item.article.id, item.heading?.id ?? null);
                }}
              >
                <span className="text-base text-text">{item.heading?.text ?? item.article.title}</span>
                <span className="text-sm text-text-muted">{item.heading === undefined ? t('help.search.article') : item.article.title}</span>
              </button>
            </li>
          ))}
        </ul>
      );
  }

  const here = guide?.articles.filter((candidate) => articleAvailable(candidate) && belongsTo(candidate, route)) ?? [];
  return (
    <div className="flex h-full min-h-0 flex-col">
    {ownPage && wanted === null && <div className="shrink-0 px-4 pt-6 pb-3"><PageHeader title={t('views.help.title')} /></div>}
    <HelpPanel
      className="min-h-0 flex-1"
      search={search}
      label={t('views.help.title')}
      value={shownPart}
      onValueChange={selectPart}
      {...(covering === undefined ? {} : { open: covering })}
      tabs={[
        ...(ownPage || about === undefined ? [] : [{ value: 'screen' as const, label: t('help.parts.screen'), icon: <Lightbulb aria-hidden />, panel: <ThisScreen about={about} articles={here} onArticle={(id: string) => open(id)} /> }]),
        { value: 'articles', label: t('help.parts.articles'), icon: <BookOpen aria-hidden />, count: guide?.articles.filter((article) => articleAvailable(article) && article.section !== 'build-your-own').length, panel: <Articles guide={guide} onArticle={(id) => open(id)} /> },
        { value: 'build', label: t('helpProvider.buildYourOwn'), icon: <BookOpen aria-hidden />, count: guide?.articles.filter((article) => articleAvailable(article) && article.section === 'build-your-own').length, panel: <Articles guide={guide} section="build-your-own" onArticle={(id) => open(id)} /> },
        { value: 'settings', label: t('helpRelease.settings'), icon: <SlidersHorizontal aria-hidden />, count: settingDefs.length, panel: <ul className="flex flex-col p-2">{settingDefs.map((def) => <li key={def.key}><button type="button" className="focus-inset flex w-full flex-col items-start rounded-md px-2 py-2 text-left hover:bg-bg-muted" onClick={() => open(settingArticle(def.key))}><span className="text-base text-text">{def.title}</span><span className="text-sm text-text-muted">{def.description}</span></button></li>)}</ul> },
        { value: 'tutorials', label: t('help.parts.tutorials'), icon: <GraduationCap aria-hidden />, panel: <TutorialList /> },
        { value: 'keys', label: t('help.parts.keys'), icon: <Keyboard aria-hidden />, panel: <div className="p-4"><KeyList /></div> },
        { value: 'news', label: t('helpRelease.news.title'), icon: <Newspaper aria-hidden />, panel: <WhatsNew /> },
      ]}
    />
    <div ref={dock} className="shrink-0" data-testid="tutorial-dock" />
    </div>
  );
}

/** What this screen is for, the hints on it, and the articles that belong to it. */
function ThisScreen({ about, articles, onArticle }: { about: TranslationKey; articles: readonly Article[]; onArticle: (id: string) => void }) {
  const { t, i18n } = useTranslation();
  const hints = useMountedHints();
  const dismissed = useDismissed();
  return (
    <div className="flex flex-col gap-6 p-4">
      <section aria-label={t('help.screen.about')} className="flex flex-col gap-1">
        <h2 className="text-md font-semibold text-text">{t('help.screen.about')}</h2>
        <p className="text-base text-text-muted">{t(about)}</p>
      </section>
      {hints.length > 0 && (
        <section aria-label={t('help.screen.hints')} className="flex flex-col gap-2">
          <h2 className="text-md font-semibold text-text">{t('help.screen.hints')}</h2>
          <ul className="flex flex-col gap-2">
            {hints.map((id) => {
              const hint = hintOf(id, t, i18n.resolvedLanguage ?? i18n.language);
              return (
                <li key={id} className="flex flex-col">
                  <span className="text-base font-medium text-text">{hint.title}</span>
                  <span className="text-sm text-text-muted">{hint.what}</span>
                </li>
              );
            })}
          </ul>
        </section>
      )}
      <section aria-label={t('help.screen.articles')} className="flex flex-col gap-2">
        <h2 className="text-md font-semibold text-text">{t('help.screen.articles')}</h2>
        <ul className="flex flex-col">
            {articles.map((article) => (
              <li key={article.id}>
                <button type="button" className="focus-inset w-full rounded-md px-2 py-2 text-left text-base text-accent-text hover:bg-bg-muted" onClick={() => onArticle(article.id)}>
                  {article.title}
                </button>
              </li>
            ))}
        </ul>
      </section>
      <Button size="sm" variant="ghost" className="self-start" onClick={() => dismissed.reopenAll()}>
        {t('help.screen.tipsBack')}
      </Button>
    </div>
  );
}

/** Every article of the guide. */
export function Articles({ guide, onArticle, section }: { guide: Guide | undefined; onArticle: (id: string) => void; section?: 'build-your-own' }) {
  if (guide === undefined) return <SkeletonRows rows={4} className="p-3" />;
  return (
    <ul className="flex flex-col p-2">
      {guide.articles.filter((article) => articleAvailable(article) && article.section === section).map((article) => (
        <li key={article.id}>
          <button type="button" className="focus-inset flex w-full flex-col items-start rounded-md px-2 py-2 text-left hover:bg-bg-muted" onClick={() => onArticle(article.id)}>
            <span className="text-base text-text">{article.title}</span>
          </button>
        </li>
      ))}
    </ul>
  );
}

/** The help as a view of the workbench. */
export function HelpView() {
  return <HelpContent />;
}
