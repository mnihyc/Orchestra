/** Short walks on the real screen. Only the person clicks, writes, grants or sends. */
import { useRouter } from '@tanstack/react-router';
import { GraduationCap } from 'lucide-react';
import { useEffect } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { useDismissed } from '@/features/help/Dismissed';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { useHelpState } from '@/features/help/state';
import { beginTutorial, leaveTutorial, useTutorialRun } from '@/features/help/tutorialState';
import { TARGETS } from '@/help/targets';
import { useLocalizedGuide } from '@/help/localized';
import { TUTORIALS, tutorialOf, type TutorialDefinition } from '@/help/tutorials';
import { boundedStep, nextStep } from '@/help/tutorials/progress';
import { TutorialExample } from '@/features/help/TutorialExample';
import { TutorialPlaces } from '@/features/help/TutorialPlaces';
import { Button } from '@/ui/Button';
import { Spotlight, useSpotlightTarget } from '@/ui/Spotlight';

/** A walk may be started from its article, from this screen or from the full list. */
export function TutorialList({ article }: { article?: string }) {
  const { t } = useTranslation();
  const router = useRouter();
  const guide = useLocalizedGuide();
  const dismissed = useDismissed();
  const origin = useHelpState((state) => state.origin);
  const userId = useStore(client.session, (state) => state.userId);
  const tutorials = article === undefined ? TUTORIALS : TUTORIALS.filter((one) => one.article === article);
  return <ul className="flex flex-col gap-3 p-3" data-testid="help-tutorials">{tutorials.map((tutorial) => {
    const finished = dismissed.isFinished(tutorial.id);
    const step = finished ? 0 : dismissed.stepOf(tutorial.id);
    const guideTitle = guide?.articles.find((one) => one.id === tutorial.article)?.title;
    return <li key={tutorial.id} className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <h3 className="flex items-center gap-2 text-base font-medium"><GraduationCap aria-hidden />{t(tutorial.title)}</h3>
      <p className="text-sm text-text-muted">{t(tutorial.about)}</p>
      {tutorial.id === 'first-thread' && <TopicPicture />}
      {finished && <span className="text-xs text-text-muted">{t('helpRelease.player.finished')}</span>}
      <div className="flex flex-wrap gap-2"><Button size="sm" disabled={userId === null} onClick={(event) => {
        if (userId === null) return;
        const fromHelp = router.state.location.pathname.startsWith('/help');
        const from = fromHelp ? origin : router.state.location.pathname;
        beginTutorial(tutorial.id, step, userId, from, event.currentTarget);
        useHelpState.setState({ overlay: false });
        const screen = tutorial.steps[boundedStep(step, tutorial.steps.length)]?.screen;
        if (fromHelp && from !== null) router.history.push(from);
        else if (fromHelp && screen !== undefined) router.history.push(screen);
      }}>{t(finished ? 'helpRelease.player.again' : step > 0 ? 'helpRelease.player.resume' : 'helpRelease.player.start')}</Button>
      {article !== tutorial.article && guideTitle !== undefined && <ArticleGuideLink article={tutorial.article} title={guideTitle} />}</div>
    </li>;
  })}</ul>;
}

/** The first lesson shows the product's two conversations, as native interface elements. */
function TopicPicture() {
  const { t } = useTranslation();
  return <figure className="rounded-md bg-bg-subtle p-3 text-sm" aria-label={t('helpRelease.player.picture')}>
    <div className="grid grid-cols-2 gap-3"><div className="rounded-lg border border-border p-2"><span className="font-medium">{t('access.kinds.chat')}</span><p className="mt-1 text-text-muted">{t('helpRelease.player.picturePeople')}</p></div><div className="rounded-lg border border-border p-2"><span className="font-medium">{t('access.kinds.thread')}</span><p className="mt-1 text-text-muted">{t('helpRelease.player.pictureAgent')}</p></div></div>
    <figcaption className="mt-2 text-xs text-text-muted">{t('helpRelease.player.picture')}</figcaption>
  </figure>;
}

/** Mounted once, so leaving Help or changing the screen does not lose the active step. */
export function TutorialHost() {
  const run = useTutorialRun();
  const userId = useStore(client.session, (state) => state.userId);
  useEffect(() => {
    if (run.id !== null && run.owner !== userId) useTutorialRun.setState({ id: null, opener: null });
  }, [run.id, run.owner, userId]);
  const tutorial = run.id === null ? undefined : tutorialOf(run.id);
  return tutorial === undefined || userId !== run.owner ? null : <ActiveTutorial key={tutorial.id} tutorial={tutorial} />;
}

function ActiveTutorial({ tutorial }: { tutorial: TutorialDefinition }) {
  const { t } = useTranslation();
  const router = useRouter();
  const guide = useLocalizedGuide();
  const dismissed = useDismissed();
  const run = useTutorialRun();
  const index = boundedStep(run.step, tutorial.steps.length);
  const step = tutorial.steps[index];
  const target = useSpotlightTarget(step === undefined ? null : TARGETS[step.target].selector);
  const shownArticle = useHelpState((state) => state.article);
  useEffect(() => { target?.element.scrollIntoView({ block: 'nearest' }); }, [target?.element]);
  useEffect(() => {
    const element = target?.element;
    if (element === undefined || step?.advanceOn !== 'click') return;
    const clicked = (event: MouseEvent) => {
      if (!(event.target instanceof Node) || !element.contains(event.target)) return;
      const outcome = nextStep(index, tutorial.steps.length, true);
      if (outcome === 'finished') { dismissed.finish(tutorial.id); leaveTutorial(); }
      else { dismissed.keepStep(tutorial.id, outcome); useTutorialRun.setState({ step: outcome }); }
    };
    // Observe the person's act before it opens a modal and hides its originating control.
    document.addEventListener('click', clicked, true);
    return () => document.removeEventListener('click', clicked, true);
  }, [target?.element, step?.advanceOn, index, tutorial, dismissed]);
  if (step === undefined) return null;
  const illustrated = target === null && step.example !== undefined;
  const ready = target !== null || illustrated;
  const move = (to: number) => { dismissed.keepStep(tutorial.id, to); useTutorialRun.setState({ step: to }); };
  const next = () => {
    const outcome = nextStep(index, tutorial.steps.length, ready);
    if (outcome === 'finished') { dismissed.finish(tutorial.id); leaveTutorial(); }
    else move(outcome);
  };
  const screen = step.screen ?? run.origin;
  const dialog = target?.element.closest('[role="dialog"]') ?? null;
  const dock = dialog === null ? run.dock : null;
  const ownGuide = router.state.location.pathname.replace(/\/$/, '') === `/help/${tutorial.article}`
    || dock !== null && shownArticle === tutorial.article;
  const guideTitle = guide?.articles.find((one) => one.id === tutorial.article)?.title;
  const atTop = target !== null && target.rect.top + target.rect.height > window.innerHeight / 2;
  const panel = <section role="region" aria-label={t('helpRelease.player.title')} data-testid="tutorial-player"
    className={`${dock !== null ? 'relative max-h-[60dvh] shrink-0 overflow-y-auto' : dialog === null ? `fixed inset-x-3 ${atTop ? 'top-3' : 'bottom-3'} z-50 sm:left-auto sm:w-96 max-h-[80dvh] overflow-y-auto` : 'relative z-50 mt-3'} flex flex-col gap-3 rounded-lg border border-border-strong bg-surface p-4 shadow-lg`}>
    <header className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-base font-semibold">{t(tutorial.title)}</h2><span className="text-xs text-text-muted" role="status">{t('helpRelease.player.step', { step: index + 1, total: tutorial.steps.length })}</span></header>
    <p className="text-sm text-text" aria-live="polite">{t(step.text)}</p>
    {illustrated && step.example !== undefined && <TutorialExample kind={step.example} />}
    {step.destination !== undefined && <TutorialPlaces destination={step.destination} />}
    {target === null && !illustrated && <p className="text-sm text-text-muted" role="status">{t('helpRelease.player.missing')}</p>}
    <div className="flex flex-wrap items-center gap-2">
      <Button size="sm" disabled={index === 0} onClick={() => move(index - 1)}>{t('helpRelease.player.back')}</Button>
      <Button size="sm" variant="primary" disabled={!ready} onClick={next}>{t(index + 1 === tutorial.steps.length ? 'helpRelease.player.finish' : illustrated ? 'helpScreenReview.continueExample' : 'helpRelease.player.next')}</Button>
      <Button size="sm" variant="ghost" onClick={() => { dismissed.keepStep(tutorial.id, index); leaveTutorial(); }}>{t('helpRelease.player.leave')}</Button>
      {target === null && screen !== null && <Button size="sm" onClick={() => router.history.push(screen)}>{t('helpRelease.player.open')}</Button>}
      {!ownGuide && guideTitle !== undefined && <ArticleGuideLink article={tutorial.article} title={guideTitle} />}
    </div>
  </section>;
  return <><Spotlight target={target} />{createPortal(panel, dock ?? dialog ?? document.body)}</>;
}
