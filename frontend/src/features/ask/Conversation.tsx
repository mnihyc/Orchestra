/** Ask answers use the ordinary Markdown, reference chips, attachments and hover actions. */
import { Copy, RotateCcw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { AttachmentChips } from '@/features/composer/Attachments';
import { AgentReferences } from '@/features/people/AgentReferences';
import { TypedChip, REFERENCE_CHIP_CLASS } from '@/features/people/NamedChip';
import { referenceOfAddress } from '@/features/people/references';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { AskCitation, AskMessage } from '@/generated/api-types';
import { linkedObject, markdownReference } from '@/lib/links';
import { readOrchLink } from '@/lib/orchLinks';
import { copyText } from '@/ui/clipboard';
import { IconButton } from '@/ui/IconButton';
import { Markdown } from '@/ui/Markdown';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip, type UserChipProps } from '@/ui/UserChip';

export function AskCitationChip({ citation }: { citation: AskCitation }) {
  const { t } = useTranslation();
  const url = readOrchLink(citation.url);
  if (url === null || citation.title === null) return <span className="text-sm text-text-muted">{t('ask.sourceUnavailable')}</span>;
  const reference = referenceOfAddress(url.href);
  if (reference !== null) return <TypedChip reference={{ ...reference, label: citation.title }} />;
  const object = linkedObject(url.href), name = citation.title;
  if (object !== null) {
    const named = { id: object.id, name };
    if (object.kind === 'provider') return <ResourceChip subject={{ kind: 'provider', provider: named }} hint={false} />;
    if (object.kind === 'environment') return <ResourceChip subject={{ kind: 'environment', environment: named }} hint={false} />;
    if (object.kind === 'host') return <ResourceChip subject={{ kind: 'host', host: named }} hint={false} />;
    if (object.kind === 'agent') return <ResourceChip subject={{ kind: 'agent', agent: named }} hint={false} />;
    if (object.kind === 'template') return <ResourceChip subject={{ kind: 'template', template: named }} hint={false} />;
    if (object.kind === 'mount') return <ResourceChip subject={{ kind: 'mount', mount: named }} hint={false} />;
    if (object.kind === 'mcp_server') return <ResourceChip subject={{ kind: 'mcp_server', server: named }} hint={false} />;
    if (object.kind === 'skill') return <ResourceChip subject={{ kind: 'skill', skill: named }} hint={false} />;
  }
  return <a className={REFERENCE_CHIP_CLASS} href={`${url.pathname}${url.search}${url.hash}`}>{name}</a>;
}

/** Whether an Ask message is a step that wrote nothing: it only read sources on the way to an answer. */
export function readOnlyStep(message: AskMessage): boolean {
  return message.role === 'assistant' && message.content.every((part) => part.type === 'text' && part.text.trim() === '');
}

export function AskMessageRow({ message, threadId, owner, canRetry, onRetry }: { message: AskMessage; threadId: string; owner?: UserChipProps['user']; canRetry: boolean; onRetry: () => void }) {
  const { t } = useTranslation();
  const text = message.content.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n');
  const attachments = message.content.filter((part) => part.type !== 'text');
  const copied = text + (message.citations.length === 0 ? '' : `\n\n${message.citations.filter((one) => one.title !== null).map((one) => markdownReference(one.title ?? '', one.url)).join('\n')}`);
  return <li className="group/ask-message flex min-w-0 flex-col gap-2 py-3" data-ask-message={message.id} data-role={message.role}>
    <div className="flex min-w-0 items-center gap-2 text-xs text-text-muted">{message.role === 'user' && owner !== undefined ? <UserChip user={owner} /> : <span>{t(message.role === 'user' ? 'ask.question' : 'ask.title')}</span>}<RelativeTime at={message.created_at} /></div>
    {message.complete && readOnlyStep(message) ? <span className="text-sm text-text-muted">{t('ask.noAnswer')}</span>
      : <AgentReferences threadId={threadId}><Markdown text={text} streaming={message.role === 'assistant' && !message.complete} breaks compact /></AgentReferences>}
    {attachments.length > 0 && <AttachmentChips parts={attachments} />}
    {message.citations.length > 0 && <ul className="flex min-w-0 flex-wrap items-center gap-2 text-sm" aria-label={t('ask.sources')}>
      {message.citations.map((citation) => <li className="min-w-0 max-w-full" key={citation.reference}><AskCitationChip citation={citation} /></li>)}
    </ul>}
    {message.role === 'assistant' && <div className="flex h-(--control-sm) items-center gap-1 opacity-0 transition-opacity group-focus-within/ask-message:opacity-100 group-hover/ask-message:opacity-100 [@media(hover:none)]:opacity-100">
      <IconButton size="sm" label={t('ask.copy')} icon={<Copy aria-hidden />} onClick={() => void copyText(copied, t('common.copied'))} />
      <IconButton size="sm" label={t('ask.retry')} icon={<RotateCcw aria-hidden />} disabled={!canRetry} onClick={onRetry} />
    </div>}
  </li>;
}
