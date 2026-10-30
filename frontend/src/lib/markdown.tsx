/** Safe Markdown rendering without raw HTML.
 *
 * What an agent or a person wrote as Markdown is read into blocks and drawn as elements, one by one:
 * nothing is ever turned into a string of markup and put into the page, so no text can bring markup
 * of its own. Raw HTML in the text is shown as the text it is. A link goes only to an address that is
 * safe to follow (web, mail, or this application's own), and an image is a link to it and is never
 * fetched by the page itself.
 */
import { Lexer, type Token, type Tokens } from 'marked';
import { createContext, type ReactNode, useContext } from 'react';
import { i18n } from '@/i18n';
import { messageLinkAllowed, messageLinkTarget } from '@/features/people/links';

import { CodeBlock } from '@/ui/CodeBlock';
import { cx } from '@/ui/cx';
import { ScrollRegion } from '@/ui/ScrollRegion';

const OWN_ADDRESS = /^(\/(?![/\\])|#|\.\.?\/)/;
/** A link that leaves the application opens in a tab of its own and tells that tab nothing of this page. */
const NEW_TAB = { target: '_blank', rel: 'noopener noreferrer' } as const;

/**
 * Where a local address written in a message really leads, for the reader of a conversation whose environment has
 * its ports: `http://localhost:3000/…` is the environment's port 3000 reached through Orch, not the reader's own
 * machine. Null (no place supplies it, or the address names no port there) keeps the address as written.
 */
export const LocalAddressRewrite = createContext<((href: string) => string | null) | null>(null);

/**
 * How a link, or a text in backquotes, that names a thing of Orch is drawn where an agent wrote the text: as the chip of
 * what it names, resolved as the agent's thread resolves it. Null (a person's text, a page with no agent) keeps links
 * and code as they were written, and so does a drawer that gives null for one of them.
 */
export const ReferenceDrawer = createContext<((target: string, code: boolean, fallback: ReactNode) => ReactNode | null) | null>(null);

/** Inline code, or the chip of the reference it is where the text's agent named one so. */
function CodeSpan({ text }: { text: string }) {
  const draw = useContext(ReferenceDrawer);
  const code = <code className="rounded-sm bg-bg-muted px-1 py-1 break-words">{text}</code>;
  return draw?.(text, true, code) ?? code;
}

/** A link of a message: the chip of the reference it is, where the text's agent named one; else a link to follow where
 * its address is safe, and its words otherwise. */
function LinkView({ href, title, children }: { href: string; title?: string; children: ReactNode }) {
  const draw = useContext(ReferenceDrawer);
  const drawn = draw?.(href, false, <span>{children}</span>) ?? null;
  if (drawn !== null) return drawn;
  const target = messageLinkTarget(href);
  if (target === null) return <span>{children}</span>;
  return (
    <MessageLink href={target} {...(title ? { title } : {})}>
      {children}
    </MessageLink>
  );
}

/** A link of a message, which follows a local address to the environment's port where the conversation says so. */
function MessageLink({ href, title, children }: { href: string; title?: string; children: ReactNode }) {
  const rewrite = useContext(LocalAddressRewrite);
  const target = rewrite?.(href) ?? href;
  const external = !OWN_ADDRESS.test(target.trim());
  return (
    <a
      href={target}
      {...(title ? { title } : {})}
      {...(external ? NEW_TAB : {})}
      className="text-accent-text underline underline-offset-2 hover:text-accent-hover"
    >
      {children}
    </a>
  );
}

/** Whether a link's address is one to follow. */
export function isSafeHref(href: string): boolean {
  return messageLinkAllowed(href);
}

/** Reads text into blocks. Raw HTML is kept in the tokens, as text, and never turned into elements. `breaks`: a line break inside a paragraph is kept, as in what a person wrote. */
export function lexMarkdown(source: string, breaks = false): Token[] {
  return Lexer.lex(source, { gfm: true, breaks, async: false });
}

/** What is added to the end of the last run of text: the caret of a message that is still being written. */
export interface Tail {
  node: ReactNode;
}

function inline(tokens: readonly Token[] | undefined, key: string, tail?: Tail): ReactNode[] {
  const out: ReactNode[] = [];
  (tokens ?? []).forEach((token, index) => {
    const at = `${key}.${index}`;
    switch (token.type) {
      case 'text': {
        const text = token as Tokens.Text;
        if (text.tokens !== undefined && text.tokens.length > 0) out.push(...inline(text.tokens, at));
        else out.push(text.text);
        break;
      }
      case 'escape':
        out.push((token as Tokens.Escape).text);
        break;
      case 'strong':
        out.push(<strong key={at}>{inline((token as Tokens.Strong).tokens, at)}</strong>);
        break;
      case 'em':
        out.push(<em key={at}>{inline((token as Tokens.Em).tokens, at)}</em>);
        break;
      case 'del':
        out.push(<del key={at}>{inline((token as Tokens.Del).tokens, at)}</del>);
        break;
      case 'codespan':
        out.push(<CodeSpan key={at} text={(token as Tokens.Codespan).text} />);
        break;
      case 'br':
        out.push(<br key={at} />);
        break;
      case 'link': {
        const link = token as Tokens.Link;
        out.push(
          <LinkView key={at} href={link.href} {...(link.title ? { title: link.title } : {})}>
            {inline(link.tokens, at)}
          </LinkView>,
        );
        break;
      }
      case 'image': {
        // Never fetched by the page: a link to it, under the words that describe it.
        const image = token as Tokens.Image;
        const label = image.text === '' ? image.href : image.text;
        const target = messageLinkTarget(image.href);
        out.push(
          target !== null ? (
            <a key={at} href={target} {...NEW_TAB} className="text-accent-text underline underline-offset-2">
              {label}
            </a>
          ) : (
            <span key={at}>{label}</span>
          ),
        );
        break;
      }
      case 'checkbox':
        break;
      default:
        // Inline HTML and anything a newer parser adds: the text it was written as.
        out.push((token as { raw?: string }).raw ?? '');
    }
  });
  if (tail !== undefined) out.push(<span key={`${key}.tail`}>{tail.node}</span>);
  return out;
}

const HEADING: Record<number, string> = {
  1: 'text-xl font-semibold',
  2: 'text-lg font-semibold',
  3: 'text-md font-semibold',
  4: 'text-md font-semibold',
  5: 'text-md font-medium',
  6: 'text-md font-medium text-text-muted',
};

/** Draws blocks. `tail` is put at the very end of the last block that holds text. */
export function renderBlocks(tokens: readonly Token[], key = 'b', tail?: Tail): ReactNode[] {
  const out: ReactNode[] = [];
  const meaningful = tokens.filter((token) => token.type !== 'space' && token.type !== 'def');
  let lastText = -1;
  meaningful.forEach((token, index) => {
    if (['paragraph', 'heading', 'text'].includes(token.type)) lastText = index;
  });
  // The tail follows the last block of text when that is the last block; else it stands after all.
  const tailInside = tail !== undefined && lastText === meaningful.length - 1;

  meaningful.forEach((token, index) => {
    const at = `${key}.${index}`;
    const withTail = tailInside && index === lastText ? tail : undefined;
    switch (token.type) {
      case 'paragraph':
        out.push(
          <p key={at} className="wrap-anywhere">
            {inline((token as Tokens.Paragraph).tokens, at, withTail)}
          </p>,
        );
        break;
      case 'text': {
        const text = token as Tokens.Text;
        out.push(
          <p key={at} className="wrap-anywhere">
            {text.tokens !== undefined ? inline(text.tokens, at, withTail) : text.text}
          </p>,
        );
        break;
      }
      case 'heading': {
        const heading = token as Tokens.Heading;
        const Tag = `h${Math.min(6, Math.max(1, heading.depth))}` as 'h1';
        out.push(
          <Tag key={at} className={cx('wrap-anywhere text-text', HEADING[heading.depth] ?? HEADING[6])}>
            {inline(heading.tokens, at, withTail)}
          </Tag>,
        );
        break;
      }
      case 'code': {
        const code = token as Tokens.Code;
        out.push(<CodeBlock key={at} code={code.text} lang={code.lang ?? null} />);
        break;
      }
      case 'blockquote':
        out.push(
          <blockquote key={at} className="flex flex-col gap-2 border-l-2 border-border-strong pl-3 text-text-muted">
            {renderBlocks((token as Tokens.Blockquote).tokens, at)}
          </blockquote>,
        );
        break;
      case 'list': {
        const list = token as Tokens.List;
        const Tag = list.ordered ? 'ol' : 'ul';
        out.push(
          <Tag
            key={at}
            {...(list.ordered && typeof list.start === 'number' && list.start !== 1 ? { start: list.start } : {})}
            className={cx('flex flex-col gap-1 pl-6', list.ordered ? 'list-decimal' : 'list-disc')}
          >
            {list.items.map((item, itemIndex) => (
              <li key={`${at}.${itemIndex}`} className={cx(item.task && 'list-none')}>
                {item.task && (
                  <input type="checkbox" checked={item.checked === true} readOnly disabled className="mr-2 align-middle" aria-label={item.text} />
                )}
                {item.tokens.some((inner) => inner.type === 'paragraph' || inner.type === 'list' || inner.type === 'code')
                  ? renderBlocks(item.tokens, `${at}.${itemIndex}`)
                  : inline(item.tokens.flatMap((inner) => (inner.type === 'text' ? ((inner as Tokens.Text).tokens ?? [inner]) : [inner])), `${at}.${itemIndex}`)}
              </li>
            ))}
          </Tag>,
        );
        break;
      }
      case 'table': {
        const table = token as Tokens.Table;
        const name = table.header.map((cell) => cell.text).join(', ');
        const alignOf = (align: 'left' | 'center' | 'right' | null) => (align === 'center' ? 'text-center' : align === 'right' ? 'text-right' : 'text-left');
        out.push(
          <ScrollRegion key={at} kind="table" label={name === '' ? i18n.t('ui.scrollRegion.table') : name}>
            <table className="border-collapse text-sm">
              <thead>
                <tr>
                  {table.header.map((cell, cellIndex) => (
                    <th key={cellIndex} className={cx('border border-border bg-bg-subtle px-2 py-1 font-semibold', alignOf(cell.align))}>
                      {inline(cell.tokens, `${at}.h${cellIndex}`)}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {table.rows.map((row, rowIndex) => (
                  <tr key={rowIndex}>
                    {row.map((cell, cellIndex) => (
                      <td key={cellIndex} className={cx('border border-border px-2 py-1', alignOf(cell.align))}>
                        {inline(cell.tokens, `${at}.${rowIndex}.${cellIndex}`)}
                      </td>
                    ))}
                  </tr>
                ))}
              </tbody>
            </table>
          </ScrollRegion>,
        );
        break;
      }
      case 'hr':
        out.push(<hr key={at} className="border-border" />);
        break;
      default:
        // Block HTML, and anything unknown: the text it was written as, never markup.
        out.push(
          <p key={at} className="wrap-anywhere whitespace-pre-wrap">
            {(token as { raw?: string }).raw ?? ''}
          </p>,
        );
    }
  });
  if (tail !== undefined && !tailInside) out.push(<span key={`${key}.tail`}>{tail.node}</span>);
  return out;
}
