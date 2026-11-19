/** One result of a search as a card: where it lies, the words around the match with the match marked, who wrote it and when.
 *
 * The card is a link to the thing itself: the thread at the item, the document at the block, the chat at
 * the message. It is a real link, so that a click opens it here and a click with the usual key or a middle
 * click opens it in a new tab of the browser.
 */
import { ThreadStateIcon } from '@/ui/ThreadState';
import { useRouter } from '@tanstack/react-router';
import { FileText, MessageSquareText, PanelRightOpen } from 'lucide-react';
import type { MouseEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { pathOf, WAY_SEPARATOR } from '@/features/finder/destinations';
import { Excerpt } from '@/features/search/Excerpt';
import { PLACE_ICONS } from '@/features/navigation/places';
import type { TypedReference } from '@/features/people/references';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { atOfHit, authorOfHit, crumbsOf, destinationOfHit } from '@/features/search/hits';
import { openResult, targetOfHit } from '@/features/search/ResultView';
import type { SearchHit } from '@/generated/api-types';
import { UserChip } from '@/ui/UserChip';
import { AgentChip } from '@/ui/AgentChip';
import { RelativeTime } from '@/ui/RelativeTime';
import { IconButton } from '@/ui/IconButton';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { SearchFiles } from '@/features/files/MessageFiles';

export interface HitProps {
  hit: SearchHit;
  foundBy?: 'words' | 'meaning' | 'both' | undefined;
  matchField?: string | null | undefined;
  slugOfOrg: (orgId: string) => string | null;
  labels?: ReadonlyMap<string, string>;
}

/** Whether a click is the plain one, which this page answers by going there itself. */
export function isPlainClick(event: MouseEvent): boolean {
  return event.button === 0 && !event.ctrlKey && !event.metaKey && !event.shiftKey && !event.altKey;
}

export function Hit({ hit, slugOfOrg, labels, foundBy, matchField }: HitProps) {
  const { t } = useTranslation();
  const router = useRouter();
  const destination = destinationOfHit(hit, slugOfOrg);
  const target = targetOfHit(hit);
  const href = destination === null ? null : pathOf(destination);
  const crumbs = crumbsOf(hit, { chat: t('search.hit.chat'), private: (kind) => t('finder.private', { kind: t(`finder.kinds.${kind}`) }) }, labels);
  const where = crumbs.slice(0, -1).join(WAY_SEPARATOR);
  const name = crumbs.at(-1) ?? '';
  const author = authorOfHit(hit);
  const kind = hit.kind === 'item' ? hit.item.kind : hit.kind === 'document' && hit.block_id === null ? 'document_title' : hit.kind;
  const field = matchField ?? ('match_field' in hit && typeof hit.match_field === 'string' ? hit.match_field : undefined);
  const location = field === 'body' && hit.kind === 'document' ? 'document' : field !== undefined && field !== 'body' ? field : kind;
  const Icon = hit.kind === 'document' ? FileText : hit.kind === 'chat_message' ? MessageSquareText : PLACE_ICONS[hit.kind === 'name' ? hit.node.kind : 'thread'];
  const reference: TypedReference = hit.kind === 'document' ? {kind:'document',id:hit.document.id,...(hit.block_id===null?{}:{block:hit.block_id})}
    : hit.kind === 'chat_message' ? {kind:'chat',id:hit.chat.id,message:hit.message.seq_created}
      : {kind:'node',id:hit.kind==='name'?hit.node.id:hit.thread.id};
  const named = hit.kind !== 'name' || !hit.node.redacted;
  const chipHref = href ?? (hit.kind==='name'?hit.href:`/${reference.kind==='document'?'doc':reference.kind==='chat'?'chat':'thread'}/${reference.id}`);
  const heading = <div className="flex min-w-0 flex-wrap items-center gap-x-1 pr-8 text-sm text-text-muted">
    {where !== '' && <span className="min-w-0 wrap-anywhere">{`${where}${WAY_SEPARATOR}`}</span>}
    {named ? <ReferenceCard reference={reference} label={name}>{(keep) => <a href={chipHref} title={name}
      onClick={(event) => {if(isPlainClick(event)){event.preventDefault();keep();}}}
      className="inline-flex max-w-full min-w-0 items-center gap-1 rounded-sm bg-bg-muted px-1 font-medium text-text hover:bg-border">
      {hit.kind === 'item' ? <ThreadStateIcon display={hit.thread.thread_display} /> : hit.kind === 'name' && hit.node.kind === 'thread' ? <ThreadStateIcon display={hit.node.thread_display} /> : <Icon aria-hidden className="size-4 shrink-0 text-text-muted"/>}<span className="truncate">{name}</span>
    </a>}</ReferenceCard> : <span className="min-w-0 truncate">{name}</span>}
    <AsAdminMark of={adminAccessOf(hit)?.place} />
  </div>;
  const body = <p className="mt-1 line-clamp-4 text-base wrap-anywhere whitespace-pre-line text-text">
    <Excerpt snippet={hit.snippet} matches={hit.matches} semanticOnly={foundBy==='meaning'} />
  </p>;
  // Attribution can open an agent card, so it is a sibling of the result link.
  const attribution = (
      <div className="mt-2 flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted">
        {/* Each part of the line holds the dot that follows it, so that a line that wraps ends with a dot and never starts with one;
            the kind is the last part and wraps by itself where even a line of its own is too narrow. */}
        {author !== null && 'username' in author && (
          <span className="inline-flex min-w-0 items-center gap-2">
            <UserChip user={author} />
            <span aria-hidden>·</span>
          </span>
        )}
        {author !== null && 'agent' in author && (
          <span className="inline-flex min-w-0 items-center gap-2">
            <AgentChip name={author.agent} thread={author.thread} />
            <span aria-hidden>·</span>
          </span>
        )}
        <span className="inline-flex items-center gap-2 whitespace-nowrap">
          <RelativeTime at={atOfHit(hit)} />
          <span aria-hidden>·</span>
        </span>
        <span className="min-w-0">{t(`search.matchLocations.${location}`, {defaultValue: t(`search.matchLocations.${kind}`)})}</span>
      </div>
  );

  const classes = 'relative rounded-lg border border-border bg-surface p-4 transition-colors hover:bg-bg-subtle';
  const contentClasses = 'focus-inset block rounded-sm pr-8';
  const beside = target === null ? null : <IconButton size="sm" variant="ghost" label={t('search.beside.open')} icon={<PanelRightOpen aria-hidden />} className="absolute top-3 right-3" onClick={() => openResult(target, destination)} />;
  if (href === null) {
    return (
      <li className={classes}>
        {heading}
        <div className={contentClasses}>{body}</div>
        {attribution}
        <SearchFiles hit={hit} />
        {beside}
      </li>
    );
  }
  return (
    <li className={classes}>
      {heading}
      <a
        href={href}
        aria-label={hit.snippet || name}
        className={contentClasses}
        onClick={(event) => {
          if (target !== null && event.button === 0 && event.altKey && !event.ctrlKey && !event.metaKey && !event.shiftKey) {
            event.preventDefault();
            openResult(target, destination);
            return;
          }
          if (!isPlainClick(event)) return;
          event.preventDefault();
          router.history.push(href);
        }}
      >
        {body}
      </a>
      {attribution}
      <SearchFiles hit={hit} />
      {beside}
    </li>
  );
}
