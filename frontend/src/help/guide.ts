/** The user guide as data: an article read from its Markdown into its title, its headings, the words it can be found by, and the screens it belongs to.
 *
 * The articles are the files of `docs/user/`, built into the application. This reads one of them; it
 * knows nothing of where they come from, so that the same reading serves the build and the tests.
 * A file may begin with a block between two lines of three dashes, which says what a title and a
 * heading cannot: `screens` (the routes it belongs to), `keywords` (more words to find it by), and an
 * authored `summary` in that body's own language.
 */
import { Lexer, Parser, TextRenderer, type Token, type Tokens } from 'marked';

export const ARTICLE_GROUPS = ['getting-started', 'work', 'access', 'resources', 'providers', 'operations', 'settings'] as const;
export type ArticleGroup = typeof ARTICLE_GROUPS[number];

export interface Heading {
  /** The anchor of the heading inside its article. */
  id: string;
  text: string;
  /** 2 to 4: the title of the article is the heading of level 1. */
  level: number;
  /** Search aliases authored for this particular section, keeping its anchor as the result. */
  keywords?: string[];
}

export interface Article {
  /** The file's name without its ending: the address of the article is `/help/<id>`. */
  id: string;
  title: string;
  /** A short summary authored in this article's own language, never generated from its body. */
  summary?: string;
  headings: Heading[];
  keywords: string[];
  /** The routes it belongs to, as the table of routes writes them (`/inbox`, `/thread/$threadId`). */
  screens: string[];
  /** Implementer articles appear together under Build your own. */
  section?: 'build-your-own';
  /** The catalogue section, shared by all authored languages of this article. */
  group?: ArticleGroup;
  /** Relative Markdown links in an implementer directory have a separate article namespace. */
  linkPrefix?: string;
  /** Static contract-schema assets shipped with this guide, keyed by their relative link. */
  files?: Readonly<Record<string, string>>;
  /** The text after the front matter, as Markdown. */
  body: string;
  /** Optional authored bodies share one article identity, rather than duplicating the Help index. */
  translations?: Readonly<Record<string, ArticleTranslation>>;
  /** The selected body's language, used to disclose a fallback honestly. */
  language?: string;
}

export type ArticleTranslation = Pick<Article, 'title' | 'summary' | 'headings' | 'keywords' | 'body'>;

/** Selects an authored body without mutating the English source or guessing a translation. */
export function articleInLanguage(article: Article, language: string): Article {
  const alternatives = article.translations ?? {};
  const requested = language.toLowerCase();
  const chosen = Object.keys(alternatives).find((one) => one.toLowerCase() === requested)
    ?? Object.keys(alternatives).find((one) => one.split('-')[0]?.toLowerCase() === requested.split('-')[0]);
  const text = chosen === undefined ? undefined : alternatives[chosen];
  if (text === undefined) return { ...article, language: 'en' };
  // An authored translation must supply its own summary; the English text cannot fill that field.
  const localized = { ...article, ...text, keywords: [...article.keywords, ...text.keywords], language: chosen ?? 'en' };
  if (text.summary === undefined) delete localized.summary;
  return localized;
}

export function articleMatchesLanguage(article: Article, language: string): boolean {
  return (article.language ?? 'en').split('-')[0]?.toLowerCase() === language.split('-')[0]?.toLowerCase();
}

/** The anchor a heading gets: its words in lower case joined by dashes, as the hosts of Markdown make them. */
export function slugOf(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .trim()
    .replace(/\s+/g, '-');
}

/** A list written as `a, b, c` or `[a, b, c]`. */
function listOf(value: string): string[] {
  return value
    .replace(/^\[|\]$/g, '')
    .split(',')
    .map((part) => part.trim().replace(/^["']|["']$/g, ''))
    .filter((part) => part !== '');
}

/** Splits the front matter off a source: its lines as `key: value`, and what follows. */
export function splitFrontMatter(source: string): { meta: Record<string, string>; body: string } {
  const text = source.replace(/^\uFEFF/, '').replace(/\r\n/g, '\n');
  if (!text.startsWith('---\n')) return { meta: {}, body: text };
  const end = text.indexOf('\n---', 4);
  if (end === -1) return { meta: {}, body: text };
  const meta: Record<string, string> = {};
  for (const line of text.slice(4, end).split('\n')) {
    const at = line.indexOf(':');
    if (at > 0) meta[line.slice(0, at).trim()] = line.slice(at + 1).trim();
  }
  const rest = text.slice(end + 4);
  return { meta, body: rest.startsWith('\n') ? rest.slice(1) : rest };
}

/** Reads an article. A heading inside a fenced block of code is no heading. */
export function readArticle(id: string, source: string): Article {
  const { meta, body } = splitFrontMatter(source);
  const headings: Heading[] = [];
  let title = '';
  let fenced = false;
  for (const line of body.split('\n')) {
    if (/^\s*(```|~~~)/.test(line)) {
      fenced = !fenced;
      continue;
    }
    if (fenced) continue;
    const found = /^(#{1,4})\s+(.+?)\s*#*\s*$/.exec(line);
    if (found === null) continue;
    const level = (found[1] ?? '#').length;
    const text = found[2] ?? '';
    if (level === 1) {
      if (title === '') title = text;
    } else {
      // Translated prose keeps the same destinations as the screen's question cards.
      const anchor = meta[`heading_id.${slugOf(text)}`] ?? slugOf(text);
      const aliases = meta[`heading_keywords.${anchor}`];
      headings.push({ id: anchor, text, level, ...(aliases === undefined ? {} : { keywords: listOf(aliases) }) });
    }
  }
  const group = ARTICLE_GROUPS.find((group) => group === meta.group);
  return {
    id: meta.id ?? id,
    title: meta.title ?? (title === '' ? id : title),
    ...(meta.summary === undefined ? {} : { summary: meta.summary }),
    headings,
    keywords: listOf(meta.keywords ?? ''),
    screens: listOf(meta.screens ?? ''),
    body,
    ...(group === undefined ? {} : { group }),
    ...(meta.section === 'build-your-own' ? { section: 'build-your-own' as const } : {}),
    ...(meta.link_prefix === undefined ? {} : { linkPrefix: meta.link_prefix }),
  };
}

/** The first authored prose line, with inline markup read as text rather than another link. */
export function articleFirstLine(article: Pick<Article, 'body'>): string {
  const paragraph = (tokens: readonly Token[]): Tokens.Paragraph | undefined => {
    for (const token of tokens) {
      if (isParagraph(token)) return token;
      if (isQuote(token)) { const first = paragraph(token.tokens); if (first !== undefined) return first; }
    }
    return undefined;
  };
  const first = paragraph(Lexer.lex(article.body));
  if (first === undefined) return '';
  const parser = new Parser(), renderer = new TextRenderer();
  const inline = (tokens: Token[]): string => parser.parseInline(tokens, renderer);
  renderer.strong = ({ tokens }) => inline(tokens);
  renderer.em = ({ tokens }) => inline(tokens);
  renderer.del = ({ tokens }) => inline(tokens);
  renderer.link = ({ tokens }) => inline(tokens);
  renderer.html = () => '';
  return inline(first.tokens).replace(/\s+/g, ' ').trim();
}

function isParagraph(token: Token): token is Tokens.Paragraph { return token.type === 'paragraph'; }
function isQuote(token: Token): token is Tokens.Blockquote { return token.type === 'blockquote'; }

/** A written summary where present; older implementer articles keep their existing prose preview. */
export function articlePreview(article: Pick<Article, 'summary' | 'body'>): string {
  return article.summary?.trim() || articleFirstLine(article);
}

/** The guide follows a reader's first work, then configuration and operation. */
export interface Guide {
  articles: Article[];
}

/** Known workflows have an authored order; extension articles follow them by stable id. */
const FIRST_USE = [
  'getting-started', 'connect-a-machine', 'add-a-provider', 'register-your-program', 'temporary-environments', 'message-queue',
  'workflows', 'files-and-attachments', 'finding-things', 'ask', 'who-can-see-and-do-what',
  'how-approvals-work', 'running-thread-views', 'restore-files-before-a-message', 'group-chats', 'link-sharing',
  'organization-terms', 'messages-from-admins', 'resources', 'resource-groups', 'duplicate-resources', 'mcp-import-and-use',
  'switch-and-move-environments', 'admin-inspection', 'provider-web-search', 'provider-details', 'write-an-environment-template',
  'choose-machine-storage', 'commands-after-start', 'change-a-running-environment', 'recover-lost-work',
  'let-agents-open-ports', 'settings-and-usage', 'speed-first-output-and-errors', 'updates',
  'provider-readme', 'provider-credential', 'provider-facts', 'provider-models', 'provider-resets',
  'provider-codex-plan-example', 'provider-claude-plan-example', 'provider-openrouter-key-example',
];
const firstUse = new Map(FIRST_USE.map((id, index) => [id, index]));

function readSources(sources: Readonly<Record<string, string>>): Article[] {
  return Object.entries(sources).map(([path, source]) => readArticle(path.replace(/^.*\//, '').replace(/\.md$/, ''), source));
}

export function makeGuide(sources: Record<string, string>, files: Readonly<Record<string, string>> = {}, translations: Readonly<Record<string, Readonly<Record<string, string>>>> = {}): Guide {
  const localized = Object.entries(translations).map(([language, texts]) => ({ language, articles: readSources(texts) }));
  const articles = readSources(sources)
    .map((article) => {
      const alternatives: Record<string, ArticleTranslation> = {};
      for (const one of localized) {
        const text = one.articles.find((candidate) => candidate.id === article.id);
        if (text !== undefined) alternatives[one.language] = { title: text.title, headings: text.headings, keywords: text.keywords, body: text.body,
          ...(text.summary === undefined ? {} : { summary: text.summary }) };
      }
      return Object.keys(alternatives).length === 0 ? article : { ...article, translations: alternatives };
    })
    .map((article) => article.section === 'build-your-own' ? { ...article, files } : article)
    // The README of the directory describes the guide for those who write it and is no article.
    .filter((article) => article.id.toLowerCase() !== 'readme')
    .sort((one, other) => (firstUse.get(one.id) ?? FIRST_USE.length) - (firstUse.get(other.id) ?? FIRST_USE.length) || one.id.localeCompare(other.id));
  return { articles };
}

/** Whether an article belongs to a screen: the routes it names include the route of the screen, or one it lies under. */
export function belongsTo(article: Pick<Article, 'screens'>, route: string): boolean {
  return article.screens.some((screen) => screen === route || route.startsWith(`${screen}/`));
}

/** Draft scaffolding is kept addressable but is never offered as a finished guide. */
export function articleAvailable(article: Pick<Article, 'body'>): boolean {
  return !/^Placeholder guide[.:]/mi.test(article.body) && !/^Planned coverage:/mi.test(article.body);
}
