/** Links in bundled guides retain their document namespace and resolve without a network lookup. */
export interface ArticleLink { article: string; heading: string | null }

export function resolveArticleLink(href: string, prefix = ''): ArticleLink | null {
  const help = /^\/help\/([\w-]+)(?:#([^\s]+))?$/.exec(href);
  if (help !== null) return { article: help[1] ?? '', heading: help[2] ?? null };
  if (/^(?:[a-z][a-z\d+.-]*:|\/\/|\/|#)/i.test(href)) return null;
  const file = /^([^?#]+)\.md(?:#([^\s]+))?$/.exec(href);
  if (file === null) return null;
  const path = file[1] ?? '';
  const name = path.split('/').at(-1)?.toLowerCase() ?? '';
  if (!/^[\w-]+$/.test(name)) return null;
  const namespace = path.includes('development/extensions/providers/') ? 'provider-'
    : /(?:^|\/)user\//.test(path) ? '' : prefix;
  return { article: `${namespace}${name}`, heading: file[2] ?? null };
}

/** Only authored prose has links: a worked program's string literals are not article destinations. */
export function proseLinks(markdown: string): string[] {
  let fence: string | null = null;
  const links: string[] = [];
  for (const line of markdown.split('\n')) {
    const delimiter = /^\s*(`{3,}|~{3,})/.exec(line)?.[1];
    if (delimiter !== undefined) {
      if (fence === null) fence = delimiter;
      else if (delimiter.startsWith(fence)) fence = null;
      continue;
    }
    if (fence === null) for (const found of line.matchAll(/\]\(([^\s)]+)\)/g)) if (found[1] !== undefined) links.push(found[1]);
  }
  return links;
}

export function rewriteGuideLinks(markdown: string, prefix = '', files: Readonly<Record<string, string>> = {}): string {
  let fence: string | null = null;
  return markdown.split('\n').map((line) => {
    const delimiter = /^\s*(`{3,}|~{3,})/.exec(line)?.[1];
    if (delimiter !== undefined) {
      if (fence === null) fence = delimiter;
      else if (delimiter.startsWith(fence)) fence = null;
      return line;
    }
    if (fence !== null) return line;
    return line.replace(/\]\(([^\s)]+)\)/g, (whole: string, href: string) => {
      const asset = files[href];
      if (asset !== undefined) return `](${asset})`;
      const target = resolveArticleLink(href, prefix);
      return target === null ? whole : `](/help/${target.article}${target.heading === null ? '' : `#${target.heading}`})`;
    });
  }).join('\n');
}
