/** A reference chip's card: what it names and the part it names, opened by pointer or keyboard as a person's card is, with Open, Open beside and Copy reference.
 *
 * It shows under the pointer and under the keyboard's focus alike, and a click (or Enter) keeps it open until it is
 * dismissed, the passing one giving way to it; a click with a modifier still opens the address in a new tab, as a
 * link does. It shares the chip's preview read and checks again when a known grant ends. An unreadable
 * card keeps only the shared label and offers access where the thing allows it.
 */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { File as FileIcon, FileText, LockKeyhole, MessageSquareText } from 'lucide-react';
import { HoverCard, Popover } from 'radix-ui';
import { type ReactElement, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { referencePreviewQuery } from '@/api/references';
import { ApiError } from '@/api/client';
import { AccessRequest } from '@/features/access/AccessRequest';
import { AccessCard } from '@/features/approvals/Access';
import type { Approval } from '@/generated/api-types';
import { useOpenPath } from '@/features/chat/open';
import { PLACE_ICONS } from '@/features/navigation/places';
import { type TypedReference, markdownReference, referenceTarget, referenceText } from '@/features/people/references';
import { openContent } from '@/features/workbench/contentNavigation';
import type { ReferencePreview } from '@/generated/api-types';
import { AuthorChip } from '@/ui/AuthorChip';
import { Button } from '@/ui/Button';
import { copyText } from '@/ui/clipboard';
import { Hint } from '@/features/help/Hint';
import { cardClasses, floatingAttachment, floatingClasses } from '@/ui/Popover';
import { usePromptCovered } from '@/ui/PromptScope';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton } from '@/ui/Skeleton';
import { ErrorAlert } from '@/ui/ErrorState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { ByteSize } from '@/ui/ByteSize';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useSocketStatus } from '@/realtime/live';
import { FileDetails } from '@/features/files/FileDetails';
import { FileOverlay } from '@/features/files/FileOverlay';
import { useFileVisible } from '@/features/files/FilePreview';
import { referenceSource, type FileSource } from '@/features/files/source';
import { chipSizeMenu, useChipPresentation, type ChipPresentation, type ChipSize } from '@/features/people/chipSizes';
import { ContextEntries, ContextMenu } from '@/ui/ContextMenu';
import type { MenuEntry } from '@/ui/Menu';

/** `children` draws the chip, given what keeps the card open: a plain click on the chip calls it, in place of following
 * the chip's address. */
export function ReferenceCard({ reference, source, label, children, presentation, menuEntries, onOpen, cacheScope }: { reference?: TypedReference; source?: FileSource; label: string; children: (keep: () => void, size: ChipSize) => ReactElement; presentation?: ChipPresentation; menuEntries?: (() => MenuEntry[]) | undefined; onOpen?: () => void; cacheScope?: string }) {
  const { t } = useTranslation();
  const sizes = useChipPresentation(presentation);
  const { ref: trigger, visible } = useFileVisible(source === undefined && sizes.size === 'large');
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const covered = usePromptCovered();
  const [shown, setShown] = useState(false);
  const [kept, setKept] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const restoringFocus = useRef(false);
  const chip = useRef<HTMLDivElement>(null);
  const returnToChip = useRef(false);
  // Asked once the card first opens, and kept: closing and opening it again asks nothing.
  const [asked, setAsked] = useState(false);
  const [viewing, setViewing] = useState<FileSource | null>(null);
  const entries = chipSizeMenu(t, sizes.chosen, sizes.change);
  const menu = menuEntries === undefined ? undefined : <ReferenceMenu entries={entries} more={menuEntries} />;
  const close = () => {
    if (!alive.current) return;
    setKept(false);
    setShown(false);
    setDismissed(true);
  };
  const keep = () => {
    setShown(false);
    setDismissed(false);
    setAsked(true);
    setKept(true);
  };
  const content = source !== undefined ? <div className="p-3"><FileDetails source={source} onOpen={() => setViewing(source)} /></div>
    : reference === undefined ? null : <ReferenceDetails reference={reference} cacheScope={cacheScope} enabled={asked || sizes.size === 'large' && visible} onDone={() => { close(); onOpen?.(); }} onOpenFile={setViewing} />;
  const overlay = viewing === null ? null : <FileOverlay source={viewing} onClose={() => {
    if (!alive.current) return;
    close();
    setViewing(null);
    queueMicrotask(() => {
      if (!alive.current) return;
      restoringFocus.current = true;
      trigger.current?.querySelector<HTMLElement>('a,button,[tabindex]')?.focus();
      queueMicrotask(() => { restoringFocus.current = false; });
    });
  }} />;
  if (sizes.size === 'large') return <ContextMenu label={t('fileMedia.showAs')} entries={entries} content={menu}>
    <span ref={trigger} tabIndex={0} data-chip-size="large" className="inline-flex max-w-full flex-col rounded-lg border border-border bg-surface align-top">{content}{overlay}</span>
  </ContextMenu>;
  return <ContextMenu label={t('fileMedia.showAs')} entries={entries} content={menu}><span ref={trigger} className="inline-flex max-w-full align-baseline" data-chip-size={sizes.size}>
    {source === undefined && reference !== undefined && asked && (shown || kept) && <ReferenceRead reference={reference} cacheScope={cacheScope} />}
    <Popover.Root open={kept} onOpenChange={(next) => { if (!alive.current || viewing !== null && !next) return; if (next) { setKept(true); setShown(false); setDismissed(false); } else close(); }}>
      <HoverCard.Root open={(shown || viewing !== null) && !kept && !dismissed} onOpenChange={(next) => { if (!alive.current) return; setShown(next); if (next && !dismissed) setAsked(true); }}>
        <HoverCard.Trigger asChild onPointerEnter={() => setDismissed(false)} onFocus={(event) => {
          event.preventDefault();
          if (!restoringFocus.current && alive.current) { setDismissed(false); setShown(true); setAsked(true); }
        }}>
          <Popover.Anchor data-floating-anchor asChild ref={chip}>
            {children(keep, sizes.size)}
          </Popover.Anchor>
        </HoverCard.Trigger>
        <HoverCard.Portal>
          <HoverCard.Content data-reference-card="" style={covered ? { display: 'none' } : undefined} sideOffset={6} collisionPadding={8} {...floatingAttachment} className={`z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] overflow-y-auto ${floatingClasses} ${cardClasses}`}>
            {content}
          </HoverCard.Content>
        </HoverCard.Portal>
      </HoverCard.Root>
      <Popover.Portal>
        <Popover.Content data-reference-card="" style={covered ? { display: 'none' } : undefined} sideOffset={6} collisionPadding={8} {...floatingAttachment} aria-label={label}
          onInteractOutside={(event) => {
            if (event.target instanceof Node && chip.current?.contains(event.target)) event.preventDefault();
          }}
          onEscapeKeyDown={() => { returnToChip.current = true; }}
          onCloseAutoFocus={(event) => {
            restoringFocus.current = true;
            if (returnToChip.current) { event.preventDefault(); chip.current?.focus({ preventScroll: true }); }
            returnToChip.current = false;
            queueMicrotask(() => { restoringFocus.current = false; });
          }}
          className={`z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] overflow-y-auto ${floatingClasses} ${cardClasses}`}>
          {content}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
    {overlay}
  </span></ContextMenu>;
}

/** Pinning replaces the hover portal; keep its pending read observed through that transition. */
function ReferenceRead({ reference, cacheScope }: { reference: TypedReference; cacheScope?: string }) {
  useQuery(referencePreviewQuery(reference, cacheScope));
  return null;
}

/** A row's native actions are made only when the chip menu opens. */
function ReferenceMenu({ entries, more }: { entries: MenuEntry[]; more: () => MenuEntry[] }) {
  return <ContextEntries entries={[...entries, ...more()]} />;
}

function KindIcon({ preview }: { preview: ReferencePreview }) {
  if (preview.node_kind === 'thread') return <ThreadStateIcon display={preview.thread_display} />;
  const Icon = preview.kind === 'file' || preview.kind === 'environment_file' ? FileIcon : preview.kind === 'document' ? FileText : preview.kind === 'chat' ? MessageSquareText : PLACE_ICONS[preview.node_kind ?? 'thread'];
  return <Icon aria-hidden className="mt-0.5 size-4 shrink-0 text-text-muted" />;
}

function ReferenceDetails({ reference, cacheScope, enabled, onDone, onOpenFile }: { reference: TypedReference; cacheScope?: string; enabled: boolean; onDone: () => void; onOpenFile: (source: FileSource) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const open = useOpenPath();
  const read = useQuery({ ...referencePreviewQuery(reference, cacheScope), ...(cacheScope === undefined ? {} : { staleTime: 0, refetchOnMount: 'always' as const }), enabled });
  const frame = 'flex w-[min(20rem,calc(100vw-2rem))] flex-col gap-2 p-3 text-text';
  if (read.isError) return <div className={frame}>{read.error instanceof ApiError && read.error.is('not_found') ? <DeletedReference reference={reference} /> : <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void read.refetch()}>{t('common.tryAgain')}</Button>} />}</div>;
  if (!enabled || read.data === undefined || read.isFetching && (cacheScope !== undefined || read.data.refresh_at !== undefined && read.data.refresh_at !== null)) {
    return (
      <div className={frame} aria-busy="true">
        <Skeleton className="h-4 w-2/3" />
        <Skeleton className="h-3 w-1/2" />
      </div>
    );
  }
  const preview = read.data;
  const unavailable = preview.unavailable;
  if(preview.file?.missing===true||unavailable?.deleted===true)return <div className={frame}><DeletedReference reference={reference} /></div>;
  if (unavailable!==undefined&&unavailable!==null&&reference.kind==='chat'&&unavailable.object===null) return <div className={frame}><p className="text-sm text-text-muted">{t('people.reference.noAccess')}</p></div>;
  if (unavailable !== undefined && unavailable !== null) return <div className={frame}>
    <p className="flex items-center gap-2 text-sm text-text-muted"><LockKeyhole aria-hidden className="size-4 shrink-0"/>{t(`people.reference.kinds.${reference.kind}`)}<Hint id="reference-access" /></p>
    {reference.label&&<p className="font-medium wrap-anywhere text-text-muted">{reference.label}</p>}
    <p className="text-sm text-text-muted">{t('people.reference.noAccess')}</p>
    {unavailable.requestable && unavailable.object !== null && (unavailable.permission !== null || unavailable.object.kind==='chat') && <AccessRequest
      key={`${reference.kind}:${reference.id}`}
      fixed privateReference
      membership={unavailable.object.kind==='chat'&&unavailable.permission===null}
      sharedLabel={reference.label}
      pendingId={unavailable.pending}
      missing={[{object:{...unavailable.object,name:reference.label ?? null,owner:null},permissions:unavailable.permission===null?[]:[unavailable.permission],options:null,ask:[],ask_more:0,override:false,closed:null}]}
      objectId={unavailable.object.id}
      onChanged={() => void read.refetch()}
    />}
    <ReferenceRequests preview={preview} />
  </div>;
  const target = referenceTarget(preview);
  const part = preview.part;
  const place = preview.place?.name ?? null;
  const file = preview.file ?? null;
  const enablePath=file?.enable_path;
  const fileSource = referenceSource(preview);
  const source = fileSource === null ? null : { ...fileSource, reference: referenceText(preview) };
  return (
    <div className={frame}>
      <div className="flex min-w-0 items-start gap-2">
        <KindIcon preview={preview} />
        <div className="min-w-0">
          <p className="font-medium wrap-anywhere">{preview.title}</p>
          {file === null && place !== null && <p className="text-sm wrap-anywhere text-text-muted">{place}</p>}
        </div>
      </div>
      {preview.description !== '' && <p className="line-clamp-1 text-sm wrap-anywhere text-text-muted">{preview.description.split('\n')[0]}</p>}
      {file !== null && <p className="text-sm text-text-muted wrap-anywhere" data-reference-file="">
        {file.missing ? t('people.reference.fileMissing') : <>
          {file.owner_name}
          {file.path !== null && <>{file.owner_name !== '' && <span aria-hidden> · </span>}<span className="font-mono wrap-anywhere">{file.path}</span></>}
          {(file.owner_name !== '' || file.path !== null) && <span aria-hidden> · </span>}
          {file.size === null ? <span className="tabular-nums">{t('people.reference.sizeUnknown')}</span> : <ByteSize bytes={file.size} />}
        </>}
      </p>}
      {file?.disabled===true&&<div className="flex flex-wrap items-center gap-2 text-sm text-text-muted"><EnvironmentDot status="disabled" showLabel/>{enablePath!==undefined&&enablePath!==null&&<Button size="sm" onClick={()=>{onDone();open(enablePath);}}>{t('people.reference.enableEnvironment')}</Button>}</div>}
      {source !== null && <FileDetails source={source} title={false} showMetadata={false} showPreview={part === null || !('line' in part.part) || part.text === ''} onOpen={() => onOpenFile(source)} />}
      {preview.updated_at !== null && (
        <p className="flex flex-wrap items-center gap-x-1 text-sm text-text-muted">
          {preview.updated_by === null ? t('people.reference.changed') : <>{t('agentIdentity.changedBy')} <AuthorChip author={preview.updated_by} /></>}
          <RelativeTime at={preview.updated_at} />
        </p>
      )}
      {part !== null && (
        <div className="flex flex-col gap-1 border-l-2 border-border-strong pl-2" data-reference-part="">
          <p className="text-sm text-text-muted">{'block' in part.part ? t('people.reference.block', { number: part.number }) : 'turn' in part.part ? t('people.reference.turn', { number: part.number }) : 'line' in part.part ? t('people.reference.line', { number: formatNumber(part.number, locale) }) : t('people.reference.message', { number: part.number })}</p>
          {part.text !== '' && ('line' in part.part ? <pre className="whitespace-pre-wrap font-mono text-sm wrap-anywhere">{part.text}</pre> : <p className="line-clamp-3 text-sm wrap-anywhere">{part.text}</p>)}
          {part.by !== null && <span className="text-sm"><AuthorChip author={part.by} /></span>}
        </div>
      )}
      {source === null && <div className="flex flex-wrap gap-2">
        <Hint id="references" popover />
        <Button size="sm" variant="primary" disabled={file?.missing === true} onClick={() => { onDone(); open(target); }}>{t('people.reference.open')}</Button>
        <Button size="sm" disabled={file?.missing === true} onClick={() => { onDone(); openContent(target, 'beside'); }}>{t('workbench.workspace.beside')}</Button>
        <Button size="sm" variant="ghost" onClick={() => void copyText(referenceText(preview), t('people.reference.copied'))}>{t('people.reference.copy')}</Button>
      </div>}
      <ReferenceRequests preview={preview} />
    </div>
  );
}

/** A deleted reference keeps its own copyable address without reading stale preview metadata. */
function DeletedReference({reference}:{reference:TypedReference}) {
  const {t}=useTranslation();
  const number=reference.line??reference.turn??reference.message;
  const path=reference.kind==='environment_file'?`/${reference.path??''}`:'';
  const written=markdownReference(t('people.reference.deleted'),`id:${reference.id}${path}${number===undefined?'':`#${number}`}`);
  return <>
    <p className="flex items-center gap-2 text-sm text-text-muted"><LockKeyhole aria-hidden className="size-4 shrink-0"/>{t(`people.reference.kinds.${reference.kind}`)}</p>
    <p className="font-medium text-text-muted">{t('people.reference.deleted')}</p>
    <div className="flex flex-wrap gap-2">
      <Button size="sm" disabled>{t('people.reference.open')}</Button>
      <Button size="sm" disabled>{t('workbench.workspace.beside')}</Button>
      <Button size="sm" variant="ghost" onClick={()=>void copyText(written,t('people.reference.copied'))}>{t('people.reference.copy')}</Button>
    </div>
  </>;
}

/** A decider can give access at the reference itself without being notified about it. */
function ReferenceRequests({preview}:{preview:ReferencePreview}) {
  const cache = useQueryClient();
  const live = useSocketStatus().phase === 'ready';
  const [settled,setSettled] = useState<ReadonlyMap<string,Approval>>(new Map());
  const items = (preview.access_requests??[]).filter((approval):approval is Extract<Approval,{kind:'access'}>=>approval.kind==='access');
  const onSettled = (approval:Approval)=>{setSettled((before)=>new Map(before).set(approval.id,approval));void cache.invalidateQueries({queryKey:['reference-preview']});};
  return <>{items.map((approval)=><AccessCard key={approval.id} items={[approval]} settled={settled} live={live} onSettled={onSettled} />)}</>;
}
