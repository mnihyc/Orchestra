/** Human, system, forwarded and delivered announcement inputs.
 *
 * What a person said to the agent: their picture, name and time, and what they said in a bubble.
 * Text is Markdown; an image or a file they attached is a chip with its name; what they forwarded
 * stands in the bubble as a block for each source. A line the system or an announcement put in is a
 * quiet row across the column with its icon, and has no bubble.
 */
import { useMutation } from '@tanstack/react-query';
import { Megaphone, Pencil, Settings2, Share, Target } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { continuationOutcomeUnknown, createPromptVersion } from '@/api/continuations';
import { selectThreadContinuation } from '@/api/threadRead';
import type { Author, ContentPart, Item, ThreadPromptVersionCreateBody } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { threadViewOf, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReadChanged } from '@/realtime/threadScope';
import { isForwarded } from '@/lib/content';
import { AttachmentChips } from '@/features/composer/Attachments';
import { PersonThingProvider } from '@/features/people/personContext';
import { readPastedText } from '@/lib/pastedText';
import { ItemFrame } from '@/features/timeline/ItemActions';
import { useTimelineContext } from '@/features/timeline/context';
import { PromptEditForm } from '@/features/timeline/PromptEditForm';
import { PromptNavigation } from '@/features/timeline/PromptNavigation';
import { UserChip } from '@/ui/UserChip';
import { AgentChip } from '@/ui/AgentChip';
import { FileAttachment } from '@/features/files/FileChip';
import { blobSource } from '@/features/files/source';
import { FilePathAttachment } from '@/features/files/FileLink';
import { Markdown } from '@/ui/Markdown';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { IconButton } from '@/ui/IconButton';
import { CallHelp } from '@/features/calls/Help';
import { GoalTurnBlock } from '@/features/goal/TurnBlock';
import { Tooltip } from '@/ui/Tooltip';

type UserItem = Extract<Item, { kind: 'user_message' }>;

/** One part of what was said. */
export function Part({ part, owner, updatedAt }: { part: ContentPart; owner?: Author | undefined; updatedAt?: string | undefined }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  if (isForwarded(part)) {
    const source = part.from.kind === 'chat' ? (part.from.chat.name ?? t('timeline.user.aChat')) : (part.from.thread.name ?? t('timeline.user.aThread'));
    return (
      <div className="flex flex-col gap-1 rounded-lg border-l-2 border-border-strong bg-surface px-3 py-2">
        <p className="flex items-center gap-1 text-xs text-text-subtle">
          <Share aria-hidden /> {t('timeline.user.forwarded', { source })}
        </p>
        <p className="flex items-center gap-1 text-sm text-text-muted">
          {part.author !== null && ('display_name' in part.author ? <UserChip user={part.author} size="row" /> : <AgentChip name={part.author.agent} {...(part.from.kind === 'thread' ? { thread: part.from.thread } : {})} />)}
          <TimeOfDay at={part.at} />
        </p>
        <Markdown text={part.text} compact />
        <div className="flex flex-wrap gap-1">
          {part.images.map((image) => (
            <FileAttachment key={image.blob} source={{ ...blobSource('thread', threadId, image.blob, image.name, undefined, image.media_type), owner: part.author !== null && 'id' in part.author ? part.author : undefined, updatedAt: part.at }} />
          ))}
          {part.attachments.map((file) => (
            <FileAttachment key={file.blob} source={{ ...blobSource('thread', threadId, file.blob, file.name, file.size, file.media_type), owner: part.author !== null && 'id' in part.author ? part.author : undefined, updatedAt: part.at }} />
          ))}
        </div>
      </div>
    );
  }
  switch (part.type) {
    case 'pasted_text':
      return <AttachmentChips parts={[part]} readPaste={(paste) => readPastedText('thread', threadId, paste)} />;
    case 'text':
      return part.text.trim() === '' ? null : <Markdown text={part.text} breaks />;
    case 'image':
      return <FileAttachment source={{ ...blobSource('thread', threadId, part.blob, part.name, undefined, part.media_type), owner, updatedAt }} />;
    case 'file':
      return part.blob === null ? <FilePathAttachment path={part.path} name={part.name} size={part.size} /> : <FileAttachment source={{ ...blobSource('thread', threadId, part.blob, part.name, part.size), deliveredPath: part.path, owner, updatedAt }} />;
  }
}

export function UserMessage({ item }: { item: UserItem }) {
  const { t } = useTranslation();
  const { source, parts } = item.payload;

  if (source !== 'user' || item.author === null) {
    const Icon = source === 'announcement' ? Megaphone : source === 'goal' ? Target : Settings2;
    return (
      <ItemFrame item={item} label={t('timeline.user.menu')}>
        <div className="flex items-start gap-2 rounded-md px-3 py-2 text-sm text-text-muted">
          <Icon className="mt-0.5" aria-hidden />
          <div className="min-w-0 flex-1">
            <span className="sr-only">{source === 'announcement' ? t('timeline.user.announcement') : t('timeline.user.system')}: </span>
            {source === 'goal' && item.payload.goal !== null && item.payload.goal !== undefined
              ? <GoalTurnBlock facts={item.payload.goal} />
              : parts.map((part, index) => (
                <Part key={index} part={part} owner={item.author ?? undefined} updatedAt={item.created_at} />
              ))}
          </div>
        </div>
      </ItemFrame>
    );
  }

  return <HumanMessage item={item} />;
}

function HumanMessage({ item }: { item: UserItem }) {
  const { t } = useTranslation();
  const format=useFormatContext();
  const context = useTimelineContext();
  const { threadId } = context;
  const read = useThreadReadRef(threadId);
  const mounted = useRef(false);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const { parts, delivery } = item.payload;
  const summary = useThreadPart(threadId, (view) => item.payload.input_id === null ? undefined : view.promptSummaries.get(item.payload.input_id));
  const currentTurn = useThreadPart(threadId, (view) => view.thread?.current_turn_id ?? null);
  const [editing, setEditing] = useState(false);
  const [preparationError, setPreparationError] = useState<Error | null>(null);
  const edit = useMutation({
    mutationFn: (body: ThreadPromptVersionCreateBody) => {
      if (item.payload.input_id === null) return Promise.reject(threadReadChanged());
      return createPromptVersion(read, item.payload.input_id, body);
    },
    onSuccess: (reply) => {
      if (!mounted.current) return;
      setEditing(false);
      if (context.onVersionSelect !== undefined) context.onVersionSelect(reply.continuation_id, reply.input_id === null || summary === undefined
        ? undefined : { inputId: reply.input_id, itemId: null, groupId: summary.group_id });
      else selectThreadContinuation(read, reply.continuation_id);
    },
  });
  const mine = context.inspection === undefined && summary !== undefined && summary.author?.id === context.meId;
  const unavailable = summary?.edit.available === true ? null : summary?.edit.detail
    ?? t(`messageVersions.unavailable.${summary?.edit.reason ?? 'access_required'}`);
  const retryOnly = edit.variables !== undefined && continuationOutcomeUnknown(edit.error);
  const begin = () => { if (!mine || unavailable !== null) return; setPreparationError(null); edit.reset(); setEditing(true); };
  const author = item.author;
  if (author === null) return null;
  return (
    <PersonThingProvider thing={{ kind: 'thread', id: threadId, ...(item.turn_id === null ? {} : { anchor: { turn: item.turn_id } }) }}><ItemFrame item={item} label={t('timeline.user.menu')} extra={mine ? [{ id: 'edit-message', label: t('messageVersions.edit'),
      icon: <Pencil aria-hidden />, disabled: unavailable ?? false, onSelect: begin }] : []}>
      <div className="flex items-start gap-3">
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          <p className="flex flex-wrap items-baseline gap-x-2 gap-y-1 text-sm">
            <UserChip user={author} size={28} />
            <TimeOfDay at={item.created_at} className="text-xs text-text-subtle" />
            {delivery !== 'queue' && <span className="text-xs text-text-subtle">· {delivery === 'steer' ? t(item.status === 'in_progress' ? 'composer.queue.steering' : 'composer.queue.steered') : t(`timeline.user.delivery.${delivery}`)}</span>}
            {item.payload.automatic !== null && item.payload.automatic !== undefined && <span className="inline-flex min-w-0 max-w-full items-baseline gap-1"><Tooltip content={t('calls.automaticDetail', { rule: item.payload.automatic.rule_id, attempt: formatNumber(item.payload.automatic.attempt, format.locale) })}><span tabIndex={0} className="focus-inset min-w-0 rounded-sm wrap-anywhere text-sm text-text-muted" data-automatic-input>{t('calls.automatic')}</span></Tooltip><CallHelp section="automatic" /></span>}
          </p>
          <div className="ml-10 flex min-w-0 flex-col gap-1">
          {editing ? <PromptEditForm parts={parts} activeTurnId={currentTurn} pending={edit.isPending}
            retryOnly={retryOnly} unavailable={retryOnly ? null : unavailable} error={preparationError ?? edit.error}
            onCancel={() => setEditing(false)} onSubmit={(content, activeTurn) => {
              if (retryOnly) { edit.mutate(edit.variables); return; }
              const view = threadViewOf(read);
              const projection = view.read?.projection;
              const queue = view.thread?.queue;
              if (projection === undefined || projection === null || queue === undefined) { setPreparationError(threadReadChanged()); return; }
              const body: ThreadPromptVersionCreateBody = { client_id: newActKey(), content,
                source_continuation_id: projection.viewed_continuation_id,
                expected_active_continuation_id: projection.active_continuation_id,
                expected_execution_revision: projection.execution_revision, expected_queue_revision: queue.revision,
                ...(activeTurn === undefined ? {} : { active_turn: activeTurn }) };
              edit.mutate(body);
            }}>
          </PromptEditForm> : <div className="flex max-w-full flex-col gap-2 self-start rounded-lg bg-bg-muted px-3 py-2">
            {parts.map((part, index) => (
              <Part key={index} part={part} owner={author} updatedAt={item.created_at} />
            ))}
          </div>}
          {!editing && context.inspection === undefined && (mine || (summary?.total ?? 0) > 1) && <div className="flex flex-wrap items-center gap-1">
            {summary !== undefined && <PromptNavigation threadId={threadId} prompt={summary} onSelect={context.onVersionSelect} />}
            {mine && <IconButton size="sm" label={t('messageVersions.edit')} icon={<Pencil aria-hidden />}
              blocked={unavailable !== null} reason={unavailable ?? undefined} onClick={begin} />}
          </div>}
          </div>
        </div>
      </div>
    </ItemFrame></PersonThingProvider>
  );
}
