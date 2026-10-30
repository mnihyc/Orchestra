/** Pure content validation shared by the build and regression tests. A missing link or real control is a failure. */
import { articleAvailable, belongsTo, slugOf, type Guide } from './guide';
import { screenAbout } from '../features/help/screenWords';
import { GUIDANCE } from './guidance';
import { proseLinks, resolveArticleLink } from './links';
import { PROVIDER_GUIDES } from './providers';
import { TEMPLATE_GUIDE } from './templates';
import { TASK_GUIDES } from './tasks';
import { FILE_GUIDE } from './files';
import { READING_GUIDE } from './reading';
import type { TutorialTarget } from './targets';
import type { TutorialDefinition } from './tutorials/model';

export interface HelpContentInput {
  guide: Guide;
  tutorials: readonly TutorialDefinition[];
  targets: Readonly<Record<string, TutorialTarget>>;
  sources: Readonly<Record<string, string>>;
  catalogues: readonly unknown[];
  routes: readonly string[];
  /** User articles are checked separately from extension implementer references. */
  userArticles: readonly string[];
  settings?: readonly { key: string }[];
}

export function contentWord(catalogue: unknown, key: string): string | null {
  let value: unknown = catalogue;
  for (const part of key.split('.')) {
    if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
    value = (value as Record<string, unknown>)[part];
  }
  return typeof value === 'string' && value.trim() !== '' ? value : null;
}

const codeOnly = (source: string) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
const routeName = (route: string) => route === '/' ? '/' : route.replace(/\/$/, '');

export function checkHelpContent(input: HelpContentInput): string[] {
  const failures: string[] = [];
  const article = (id: string, heading?: string, language = 'en') => {
    const found = input.guide.articles.find((one) => one.id === id);
    if (found === undefined || !articleAvailable(found)) failures.push(`article.missing: ${id}`);
    else if (heading !== undefined && !(found.translations?.[language]?.headings ?? found.headings).some((one) => one.id === heading || slugOf(one.text) === heading)) failures.push(`heading.missing: ${id}#${heading}`);
  };
  const text = (key: string) => input.catalogues.forEach((catalogue, locale) => {
    if (contentWord(catalogue, key) === null) failures.push(`text.missing: ${locale}:${key}`);
  });
  const routes = input.routes.map(routeName);
  const userArticles = input.guide.articles.filter((one) => input.userArticles.includes(one.id));
  for (const route of routes) {
    if (!userArticles.some((one) => articleAvailable(one) && one.screens.some((screen) => routeName(screen) === route))) failures.push(`screen.article_missing: ${route}`);
  }
  for (const id of input.userArticles) {
    const one = userArticles.find((entry) => entry.id === id);
    if (one === undefined) { failures.push(`article.missing: ${id}`); continue; }
    const chinese = one.translations?.['zh-CN'];
    if (chinese === undefined || !articleAvailable(chinese) || !/\p{Script=Han}/u.test(chinese.body) || chinese.body === one.body) {
      failures.push(`article.translation_missing: ${id}:zh-CN`);
      continue;
    }
    for (const heading of one.headings) {
      if (!chinese.headings.some((translated) => translated.id === heading.id)) failures.push(`article.translation_heading_missing: ${id}#${heading.id}:zh-CN`);
    }
  }
  for (const route of input.routes) {
    const about = screenAbout(route);
    if (about === undefined) continue;
    text(about);
    const normalized = route === '/' ? '/' : route.replace(/\/$/, '');
    if (!input.guide.articles.some((one) => articleAvailable(one) && belongsTo(one, normalized))) failures.push(`screen.article_missing: ${normalized}`);
  }
  for (const setting of input.settings ?? []) {
    for (const field of ['title', 'description']) text(`settingRegistry.${setting.key}.${field}`);
  }
  text(READING_GUIDE.title);
  for (const [language, headings] of Object.entries(READING_GUIDE.headings)) {
    if (language !== 'en' && input.guide.articles.find((one) => one.id === READING_GUIDE.id)?.translations?.[language] === undefined) failures.push(`article.translation_missing: ${READING_GUIDE.id}:${language}`);
    for (const heading of headings) article(READING_GUIDE.id, heading, language);
  }
  if (!(input.sources['features/workbench/Arrangements.tsx'] ?? '').includes('<ReadingGuideMenuItem')) failures.push(`guide.unlinked: ${READING_GUIDE.id}`);
  const seen = new Set<string>();
  for (const one of input.guide.articles) {
    if (seen.has(one.id)) failures.push(`article.duplicate: ${one.id}`);
    seen.add(one.id);
    const bodies = [{ language: 'en', body: one.body }, ...Object.entries(one.translations ?? {}).map(([language, text]) => ({ language, body: text.body }))];
    for (const body of bodies) for (const href of proseLinks(body.body)) {
      if (one.files?.[href] !== undefined) continue;
      const link = resolveArticleLink(href, one.linkPrefix);
      if (link !== null) article(link.article, link.heading ?? undefined, body.language);
      if (href.startsWith('schema/')) failures.push(`file.missing: ${one.id}:${href}`);
    }
    for (const screen of one.screens) {
      const wanted = routeName(screen);
      if (!routes.some((route) => route === wanted || route.startsWith(`${wanted}/`))) failures.push(`screen.missing: ${one.id}:${screen}`);
    }
  }
  for (const id of ['provider-readme', 'provider-facts', 'provider-models', 'provider-resets', 'provider-credential']) {
    const found = input.guide.articles.find((one) => one.id === id);
    if (found?.section !== 'build-your-own') failures.push(`provider.article_missing: ${id}`);
  }
  const controlGuides = new Set<string>();
  for (const [path, source] of Object.entries(input.sources)) {
    const code = codeOnly(source);
    if (path.startsWith('routes/') && /\bstaticData:\s*\{[^}]*\bscreen:/.test(code)) {
      for (const match of code.matchAll(/createFileRoute\('([^']+)'\)/g)) {
        if (match[1] !== undefined && screenAbout(match[1]) === undefined) failures.push(`screen.words_missing: ${match[1]}`);
      }
    }
    for (const link of code.matchAll(/article:\s*'([\w-]+)'(?:\s*,\s*heading:\s*'([\w-]+)')?/g)) if (link[1] !== undefined) article(link[1], link[2]);
    for (const link of code.matchAll(/\barticle="([\w-]+)"(?:\s+heading="([\w-]+)")?/g)) if (link[1] !== undefined) article(link[1], link[2]);
    // A localized anchor must exist in that language's authored article, including non-Latin headings.
    for (const pattern of [
      /<TaskGuideLink\s+guide="([\w-]+)"\s+heading=\{t\('([\w.-]+)'\)\}/g,
      /guide=\{\{\s*article:\s*'([\w-]+)',\s*heading:\s*t\('([\w.-]+)'\)\s*\}\}/g,
    ]) for (const link of code.matchAll(pattern)) {
      const id = link[1], key = link[2];
      if (id === undefined || key === undefined) continue;
      text(key);
      input.catalogues.forEach((catalogue, locale) => {
        const heading = contentWord(catalogue, key);
        if (heading !== null) article(id, heading, locale === 1 ? 'zh-CN' : 'en');
      });
    }
    for (const link of code.matchAll(/(?:<(?:GuideLink|ProviderGuideLink|TaskGuideLink)\s+guide="|\bhelp\s*[:=]\s*\{?\s*help\([^;\n]+?,\s*')([\w-]+)["']/g)) {
      if (link[1] === undefined) continue;
      article(link[1]);
      if (path.startsWith('features/') && !path.startsWith('features/help/')) controlGuides.add(link[1]);
    }
  }
  text('taskGuides.open');
  for (const task of TASK_GUIDES) {
    article(task.id);
    text(task.title);
    const found = input.guide.articles.find((one) => one.id === task.id);
    const chinese = found?.translations?.['zh-CN'];
    if (chinese === undefined || !/\p{Script=Han}/u.test(chinese.body) || chinese.body === found?.body) failures.push(`article.translation_missing: ${task.id}:zh-CN`);
    if (!controlGuides.has(task.id)) failures.push(`guide.unlinked: ${task.id}`);
  }
  for (const guide of PROVIDER_GUIDES) {
    article(guide.id);
    text(guide.title);
    if (!controlGuides.has(guide.id)) failures.push(`guide.unlinked: ${guide.id}`);
  }
  for (const heading of TEMPLATE_GUIDE.headings) article(TEMPLATE_GUIDE.id, heading);
  const fileGuide = input.guide.articles.find((one) => one.id === FILE_GUIDE.id);
  if (fileGuide?.translations?.['zh-CN'] === undefined) failures.push(`article.translation_missing: ${FILE_GUIDE.id}:zh-CN`);
  for (const section of Object.values(FILE_GUIDE.sections)) {
    article(FILE_GUIDE.id, section.en);
    article(FILE_GUIDE.id, section['zh-CN'], 'zh-CN');
    text(section.term); text(section.what);
  }
  for (const [path, section] of [
    ['features/files/FileDetails.tsx', 'previews'], ['features/files/FileOverlay.tsx', 'open'], ['features/composer/Attachments.tsx', 'agent'],
  ]) if (!codeOnly(input.sources[path ?? ''] ?? '').includes(`<FileGuideHint section="${section}"`)) failures.push(`guide.unlinked: ${FILE_GUIDE.id}:${path}`);
  text(TEMPLATE_GUIDE.title);
  for (const path of ['features/resources/templates/Template.tsx', 'features/resources/templates/NewTemplate.tsx']) {
    if (!codeOnly(input.sources[path] ?? '').includes('<TemplateGuideLink')) failures.push(`guide.unlinked: ${TEMPLATE_GUIDE.id}:${path}`);
  }
  for (const guide of GUIDANCE) {
    article(guide.id);
    text(guide.title);
    if (!controlGuides.has(guide.id)) failures.push(`guide.unlinked: ${guide.id}`);
    if (!input.tutorials.some((tutorial) => tutorial.id === guide.tutorial && tutorial.article === guide.id)) failures.push(`guide.tutorial_missing: ${guide.id}`);
  }
  for (const [id, target] of Object.entries(input.targets)) {
    if (target.evidence.length === 0) failures.push(`target.missing: ${id}`);
    for (const evidence of target.evidence) {
      const source = input.sources[evidence.source];
      if (source === undefined || !codeOnly(source).includes(evidence.marker)) failures.push(`target.missing: ${id}:${evidence.source}`);
    }
  }
  const ids = new Set<string>();
  for (const tutorial of input.tutorials) {
    if (ids.has(tutorial.id)) failures.push(`tutorial.duplicate: ${tutorial.id}`);
    ids.add(tutorial.id);
    article(tutorial.article);
    text(tutorial.title); text(tutorial.about);
    if (tutorial.steps.length === 0) failures.push(`tutorial.empty: ${tutorial.id}`);
    for (const step of tutorial.steps) {
      text(step.text);
      if (['approvals-walk', 'ports-walk', 'live-environment-walk', 'access-walk'].includes(tutorial.id)) {
        if (step.screen === undefined && step.destination === undefined) failures.push(`tutorial.no_destination: ${tutorial.id}:${step.target}`);
        if (step.example === undefined) failures.push(`tutorial.no_example: ${tutorial.id}:${step.target}`);
      }
      if (step.example !== undefined) for (const field of ['title', 'detail', 'label', 'value']) text(`helpScreenReview.examples.${step.example}.${field}`);
      if (input.targets[step.target] === undefined) failures.push(`target.missing: ${tutorial.id}:${step.target}`);
      if (step.screen !== undefined && !routes.includes(step.screen)) failures.push(`screen.missing: ${tutorial.id}:${step.screen}`);
    }
  }
  return [...new Set(failures)];
}
