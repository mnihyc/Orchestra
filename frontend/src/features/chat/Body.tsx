/** The text of a message as Markdown, with the people it names and the places it links to drawn as chips.
 *
 * What a message says is read by `markup.ts` into data and drawn here with the page's own elements, so
 * nothing a person types can become an element of its own. A name after an `@` is a chip only where
 * the server resolved it to somebody who can read the chat (`@all` where the message says so); an
 * address that leads into this application is a chip with the name of the place it leads to.
 */
import { ExternalLink } from 'lucide-react';
import { Fragment, type ReactNode } from 'react';

import { IdChip, NamedChip, SharedPathChip, TypedChip } from '@/features/people/NamedChip';
import { messageLinkTarget } from '@/features/people/links';
import { idReferenceOf, referenceOfAddress } from '@/features/people/references';
import type { UserRef } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { UserChip } from '@/ui/UserChip';

import { type Block, type Inline, parseBlocks, placeOfAddress } from '@/features/chat/markup';

export interface BodyProps {
  text: string;
  /** The people the server resolved the message's `@` names to. */
  mentions?: readonly UserRef[];
  mentionsAll?: boolean;
  /** The reader, whose own name is marked more strongly. */
  me?: string | null;
  originThread?: string | undefined;
}

/** What everybody who follows a chat is called after an `@`: the same in every language, as a username is. */
const EVERYONE = '@all';

const CHIP = 'inline-flex max-w-full items-center gap-1 rounded-sm px-1 align-baseline font-medium';

function sharedWords(runs:readonly Inline[]):string {
  return runs.map((run)=> 'text' in run ? run.text : 'children' in run ? sharedWords(run.children) : run.type==='mention' ? `@${run.name}` : '').join('');
}

function Runs({ runs, mentions, mentionsAll, me, originThread }: { runs: readonly Inline[] } & Omit<BodyProps, 'text'>): ReactNode {
  return runs.map((run, index) => {
    switch (run.type) {
      case 'text':
        return <Fragment key={index}>{run.text}</Fragment>;
      case 'break':
        return <br key={index} />;
      case 'code': {
        const written = idReferenceOf(run.text);
        if (written?.path !== undefined) return <TypedChip key={index} reference={{ kind: 'environment_file', id: written.id, path: written.path, ...(written.number === undefined ? {} : { line: written.number }) }} />;
        if (written !== null) return <IdChip key={index} id={written.id} {...(written.number === undefined ? {} : { number: written.number })} fallback={<code className="rounded-sm bg-bg-muted px-1 font-mono text-sm">{run.text}</code>} />;
        if (originThread !== undefined && (run.text.includes('/') || run.text.endsWith('.md'))) return <SharedPathChip key={index} threadId={originThread} path={run.text} code />;
        return (
          <code key={index} className="rounded-sm bg-bg-muted px-1 font-mono text-sm">
            {run.text}
          </code>
        );
      }
      case 'strong':
        return (
          <strong key={index} className="font-semibold">
            <Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} />
          </strong>
        );
      case 'em':
        return (
          <em key={index}>
            <Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} />
          </em>
        );
      case 'del':
        return (
          <del key={index}>
            <Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} />
          </del>
        );
      case 'link': {
        const label = sharedWords(run.children);
        const written = idReferenceOf(run.href);
        if (written?.path !== undefined) return <TypedChip key={index} reference={{ kind: 'environment_file', id: written.id, path: written.path, label, ...(written.number === undefined ? {} : { line: written.number }) }} />;
        if (written !== null) return <IdChip key={index} id={written.id} {...(written.number === undefined ? {} : { number: written.number })} label={label} fallback={<span><Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} /></span>} />;
        const target = messageLinkTarget(run.href, true);
        if (target === null) return <Fragment key={index}><Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} /></Fragment>;
        const typed = referenceOfAddress(run.href);
        if (typed !== null) return <TypedChip key={index} reference={{...typed,label}} />;
        const place = placeOfAddress(run.href, location.origin);
        if (place !== null) return <NamedChip key={index} {...place} original={label} />;
        if (originThread !== undefined && !/^https?:\/\//i.test(target)) return <SharedPathChip key={index} threadId={originThread} path={run.href} />;
        return (
          <a key={index} href={target} target="_blank" rel="noreferrer noopener" className="inline-flex items-baseline gap-1 text-accent-text underline underline-offset-2 hover:no-underline">
            <Runs runs={run.children} mentions={mentions} mentionsAll={mentionsAll} me={me} originThread={originThread} />
            <ExternalLink size={16} aria-hidden className="shrink-0 self-center" />
          </a>
        );
      }
      case 'mention': {
        if (run.name.toLowerCase() === 'all') {
          return mentionsAll === true ? (
            <span key={index} className={cx(CHIP, 'bg-accent-subtle text-accent-text')}>
              {EVERYONE}
            </span>
          ) : (
            <Fragment key={index}>{EVERYONE}</Fragment>
          );
        }
        const user = mentions?.find((one) => one.username.toLowerCase() === run.name.toLowerCase());
        if (user === undefined) return <Fragment key={index}>@{run.name}</Fragment>;
        return (
          <UserChip key={index} user={user} mention size={24} nameClassName={me === user.id ? 'text-text-on-accent' : 'text-accent-text'} className={cx(CHIP, me === user.id ? 'bg-accent-solid text-text-on-accent' : 'bg-accent-subtle text-accent-text')} />
        );
      }
    }
  });
}

function Blocks({ blocks, ...rest }: { blocks: readonly Block[] } & Omit<BodyProps, 'text'>) {
  return blocks.map((block, index) => {
    switch (block.type) {
      case 'paragraph':
        return (
          <p key={index} className="wrap-anywhere">
            <Runs runs={block.children} {...rest} />
          </p>
        );
      case 'code':
        return (
          // A long line scrolls inside the block; the block takes the keyboard so that it can be scrolled without a pointer.
          <pre key={index} tabIndex={0} data-scroll-allow="code" className="max-w-full overflow-x-auto rounded-lg bg-bg-muted p-3 font-mono text-sm">
            <code>{block.text}</code>
          </pre>
        );
      case 'quote':
        return (
          <blockquote key={index} className="flex flex-col gap-1 border-l-2 border-border-strong pl-3 text-text-muted">
            <Blocks blocks={block.children} {...rest} />
          </blockquote>
        );
      case 'list': {
        const List = block.ordered ? 'ol' : 'ul';
        return (
          <List key={index} className={cx('flex flex-col gap-1 pl-6', block.ordered ? 'list-decimal' : 'list-disc')}>
            {block.items.map((item, at) => (
              <li key={at} className="wrap-anywhere">
                <Runs runs={item} {...rest} />
              </li>
            ))}
          </List>
        );
      }
    }
  });
}

export function Body({ text, ...rest }: BodyProps) {
  return (
    <div className="flex min-w-0 flex-col gap-1 text-md text-text">
      <Blocks blocks={parseBlocks(text)} {...rest} />
    </div>
  );
}
