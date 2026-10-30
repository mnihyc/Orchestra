/** The Markdown a chat message may hold, read into blocks and runs of text: paragraphs, quotes, lists, code, emphasis, links and mentions, and never raw HTML.
 *
 * A message is talk, not a document, so only what talk uses is read: a paragraph with its line
 * breaks, a fenced block of code, a quotation, a list, `code`, **strong**, *emphasis*, ~~strike~~, a
 * link, a bare address, and a name after an `@`. Anything else, a heading, a table, a tag, stays the
 * characters it was typed as. What is read is data, not markup: the page draws it with its own
 * elements, so a message cannot bring an element of its own, and a link is followed only to http and https.
 */

export type Inline =
  | { type: 'text'; text: string }
  | { type: 'code'; text: string }
  | { type: 'strong'; children: Inline[] }
  | { type: 'em'; children: Inline[] }
  | { type: 'del'; children: Inline[] }
  | { type: 'link'; href: string; children: Inline[] }
  /** `@username` or `@all`; the page decides whether it names somebody who can read the chat. */
  | { type: 'mention'; name: string }
  | { type: 'break' };

export type Block =
  | { type: 'paragraph'; children: Inline[] }
  | { type: 'code'; text: string }
  | { type: 'quote'; children: Block[] }
  | { type: 'list'; ordered: boolean; items: Inline[][] };

/** How deep emphasis may be nested: more than this is a message written to break a parser, and is shown as typed. */
const MAX_DEPTH = 4;

const LINK = /^\[([^\]\n]+)\]\(((?:https?:\/\/|\/|\.\.?\/|id:)[^\s)]+)\)/;
const ADDRESS = /^https?:\/\/[^\s<>]+/;
const MENTION = /^@([A-Za-z0-9][A-Za-z0-9_-]{0,31})/;
const BULLET = /^\s{0,3}[-*]\s+(.*)$/;
const NUMBERED = /^\s{0,3}\d{1,9}[.)]\s+(.*)$/;

/** The characters an address does not end with: they belong to the sentence it stands in. */
const TRAILING = /[.,;:!?'")\]]+$/;

function isWordChar(char: string | undefined): boolean {
  return char !== undefined && /[\p{L}\p{N}_]/u.test(char);
}

function push(out: Inline[], text: string): void {
  if (text === '') return;
  const last = out.at(-1);
  if (last?.type === 'text') last.text += text;
  else out.push({ type: 'text', text });
}

/** Reads one line (or several joined by line breaks) into runs of text. */
export function parseInline(text: string, depth = 0): Inline[] {
  const out: Inline[] = [];
  let at = 0;
  while (at < text.length) {
    const char = text.charAt(at);
    const rest = text.slice(at);
    const before = at === 0 ? undefined : text.charAt(at - 1);

    if (char === '\n') {
      out.push({ type: 'break' });
      at += 1;
      continue;
    }
    if (char === '`') {
      const end = text.indexOf('`', at + 1);
      if (end > at + 1) {
        out.push({ type: 'code', text: text.slice(at + 1, end) });
        at = end + 1;
        continue;
      }
    }
    if (char === '[') {
      const link = LINK.exec(rest);
      if (link !== null) {
        out.push({ type: 'link', href: link[2] ?? '', children: parseInline(link[1] ?? '', depth + 1) });
        at += link[0].length;
        continue;
      }
    }
    if (char === 'h' && !isWordChar(before)) {
      const found = ADDRESS.exec(rest);
      if (found !== null) {
        const href = found[0].replace(TRAILING, '');
        out.push({ type: 'link', href, children: [{ type: 'text', text: href }] });
        at += href.length;
        continue;
      }
    }
    if (depth < MAX_DEPTH && (rest.startsWith('**') || rest.startsWith('~~'))) {
      const mark = rest.slice(0, 2);
      const end = text.indexOf(mark, at + 2);
      if (end > at + 2) {
        out.push({ type: mark === '**' ? 'strong' : 'del', children: parseInline(text.slice(at + 2, end), depth + 1) });
        at = end + 2;
        continue;
      }
    }
    if (depth < MAX_DEPTH && (char === '*' || char === '_') && !rest.startsWith(`${char}${char}`)) {
      const end = text.indexOf(char, at + 1);
      const inner = end > at + 1 ? text.slice(at + 1, end) : '';
      // Emphasis wraps words: not a space after its opening mark or before its closing one, and an underscore not inside a word.
      const wraps = inner !== '' && !/^\s/.test(inner) && !/\s$/.test(inner);
      const insideWord = char === '_' && (isWordChar(before) || isWordChar(text.charAt(end + 1) || undefined));
      if (wraps && !insideWord && !inner.includes('\n')) {
        out.push({ type: 'em', children: parseInline(inner, depth + 1) });
        at = end + 1;
        continue;
      }
    }
    if (char === '@' && !isWordChar(before) && before !== '@') {
      const mention = MENTION.exec(rest);
      if (mention !== null) {
        out.push({ type: 'mention', name: mention[1] ?? '' });
        at += mention[0].length;
        continue;
      }
    }
    push(out, char);
    at += 1;
  }
  return out;
}

/** Reads a message's text into blocks. */
export function parseBlocks(text: string): Block[] {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  const blocks: Block[] = [];
  let paragraph: string[] = [];
  let at = 0;

  const flush = () => {
    if (paragraph.length === 0) return;
    blocks.push({ type: 'paragraph', children: parseInline(paragraph.join('\n')) });
    paragraph = [];
  };

  while (at < lines.length) {
    const line = lines[at] ?? '';
    if (/^\s{0,3}```/.test(line)) {
      flush();
      const body: string[] = [];
      at += 1;
      while (at < lines.length && !/^\s{0,3}```\s*$/.test(lines[at] ?? '')) {
        body.push(lines[at] ?? '');
        at += 1;
      }
      blocks.push({ type: 'code', text: body.join('\n') });
      at += 1;
      continue;
    }
    if (/^\s{0,3}>/.test(line)) {
      flush();
      const quoted: string[] = [];
      while (at < lines.length && /^\s{0,3}>/.test(lines[at] ?? '')) {
        quoted.push((lines[at] ?? '').replace(/^\s{0,3}>\s?/, ''));
        at += 1;
      }
      blocks.push({ type: 'quote', children: parseBlocks(quoted.join('\n')) });
      continue;
    }
    const bullet = BULLET.exec(line);
    const numbered = bullet === null ? NUMBERED.exec(line) : null;
    if (bullet !== null || numbered !== null) {
      flush();
      const ordered = numbered !== null;
      const items: Inline[][] = [];
      while (at < lines.length) {
        const item = (ordered ? NUMBERED : BULLET).exec(lines[at] ?? '');
        if (item === null) break;
        items.push(parseInline(item[1] ?? ''));
        at += 1;
      }
      blocks.push({ type: 'list', ordered, items });
      continue;
    }
    if (line.trim() === '') flush();
    else paragraph.push(line);
    at += 1;
  }
  flush();
  return blocks;
}

/** Where an address that points into this application leads, as a path with its search; `null` for any other address. */
export function placeOfAddress(href: string, origin: string): { path: string; id: string; kind: 'org' | 'project' | 'topic' | 'thread' | 'doc' } | null {
  let url: URL;
  try {
    url = new URL(href, origin);
  } catch {
    return null;
  }
  if (url.origin !== origin) return null;
  const found = /^\/(org|project|topic|thread|doc)\/([^/]+)\/?$/.exec(url.pathname);
  if (found === null) return null;
  try {
    return { path: `${url.pathname}${url.search}`, id: decodeURIComponent(found[2] ?? ''), kind: found[1] as 'org' | 'project' | 'topic' | 'thread' | 'doc' };
  } catch {
    return null;
  }
}
