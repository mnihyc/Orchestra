/** Internal links use current readable names and kind glyphs; refused targets stay authored text; a readable chip opens its reference's card. */
import { useQuery } from '@tanstack/react-query';
import { File as FileIcon, FileText, LockKeyhole, MessageSquareText } from 'lucide-react';
import type { LucideIcon } from 'lucide-react';
import type { NodeRef } from '@/generated/api-types';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';

import { referencePreviewQuery } from '@/api/references';
import { sharedTreeQuery } from '@/api/share';
import { isPlainClick } from '@/features/chat/open';
import { PLACE_ICONS, placePath } from '@/features/navigation/places';
import { useNodeLabel, usePlaceLabel } from '@/features/navigation/placeLabels';
import type { TypedReference } from '@/features/people/references';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { idReferenceText, referenceTarget, referenceText } from '@/features/people/references';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { ORCH_REFERENCE } from '@/lib/drop';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { FileLink } from '@/features/files/FileLink';
import { FileChip as SharedFileChip } from '@/features/files/FileChip';
import { referenceSource } from '@/features/files/source';
import type { ChipPresentation } from './chipSizes';

/** What a chip names: a place of a known kind, a node whose kind the server says, a document or a chat. */
type ChipKind = 'org' | 'project' | 'topic' | 'thread' | 'node' | 'doc' | 'chat';

/** The shared compact treatment for a readable reference. */
export const REFERENCE_CHIP_CLASS = 'inline-flex max-w-full items-center gap-1 rounded-sm bg-bg-muted px-1 align-baseline font-medium text-text hover:bg-border';

/** The reference a chip names when its caller gave none: the thing whole. */
function referenceOf(kind: ChipKind, id: string): TypedReference {
  return kind === 'doc' ? { kind: 'document', id } : kind === 'chat' ? { kind: 'chat', id } : { kind: 'node', id };
}

export function NamedChip({ path, id, kind, original, reference, presentation, name }: { path: string; id: string; kind: ChipKind; original?: ReactNode; reference?: TypedReference; presentation?: ChipPresentation; name?: string }) {
  const named = reference ?? referenceOf(kind, id);
  const shared = named.label ?? (typeof original === 'string' ? original : undefined);
  // A bounded list already has this reader's permitted name; only opening its card asks for preview metadata.
  if (name !== undefined) return <ReferenceName reference={named} id={id} Icon={kind === 'doc' ? FileText : kind === 'chat' ? MessageSquareText : PLACE_ICONS[kind === 'node' ? 'thread' : kind]} path={path} name={name} presentation={presentation} />;
  return <TypedChip reference={{ ...named, ...(shared === undefined ? {} : { label: shared }) }} target={path} presentation={presentation} />;
}

/** A server-projected node name needs no row request before the shared reference card opens. */
export function KnownNodeChip({node}:{node:NodeRef}) {
  const {t}=useTranslation();
  const name=useNodeLabel(node.redacted?null:node.id,node.redacted?undefined:node.name??undefined);
  if(node.redacted || node.name===null)return <span>{t('people.privateReference')}</span>;
  return <ReferenceName path={placePath(node)} id={node.id} name={name??node.name} reference={{kind:'node',id:node.id}} Icon={PLACE_ICONS[node.kind]} />;
}

function ReferenceName({path,id,name,reference,Icon,presentation,hint=''}:{path:string;id:string;name:string;reference:TypedReference;Icon:LucideIcon;presentation?:ChipPresentation;hint?:string}) {
  return <ReferenceCard reference={reference} label={name} presentation={presentation}>
    {(keep,size)=><a href={path} draggable onDragStart={(event)=>{event.dataTransfer.setData(ORCH_REFERENCE,idReferenceText(id,name,reference.turn??reference.message??reference.line));event.dataTransfer.effectAllowed='copy';}} aria-label={name} title={name} onClick={(event)=>{if(isPlainClick(event)){event.preventDefault();keep();}}} className={REFERENCE_CHIP_CLASS}><Icon aria-hidden className="size-4 shrink-0 text-text-muted"/><span className="min-w-0 truncate">{name}</span>{size==='medium'&&hint!==''&&<span className="min-w-0 truncate text-sm font-normal text-text-muted">{hint}</span>}</a>}
  </ReferenceCard>;
}

/** A chip for a reference kept typed in a message's text: the thing it names, as its reader may read it. */
export function TypedChip({ reference, target: givenTarget, presentation }: { reference: TypedReference; target?: string; presentation?: ChipPresentation }) {
  const { t } = useTranslation();
  const placeLabel = usePlaceLabel();
  const read = useQuery(referencePreviewQuery(reference));
  const preview = read.data;
  const denied = preview?.unavailable;
  const shared = reference.label?.trim();
  const kindName = t(`people.reference.kinds.${reference.kind}`);
  const checkingExpiry=read.isFetching&&preview?.refresh_at!==undefined&&preview.refresh_at!==null;
  const locked = checkingExpiry || preview?.file?.missing===true || read.isError || denied !== undefined && denied !== null;
  const deleted = denied?.deleted === true || preview?.file?.missing===true || read.error instanceof ApiError && read.error.is('not_found');
  const name = deleted ? t('people.reference.deleted') : locked || preview === undefined ? shared===undefined||shared===''?kindName:shared : preview.kind === 'node' ? placeLabel({ id: preview.id, name: preview.title }) ?? preview.title : preview.title;
  const Icon = locked ? LockKeyhole : reference.kind === 'file' || reference.kind === 'environment_file' ? FileIcon : reference.kind === 'document' ? FileText : reference.kind === 'chat' ? MessageSquareText : PLACE_ICONS[preview?.node_kind ?? 'thread'];
  const target = preview === undefined ? '' : givenTarget ?? referenceTarget(preview);
  if (!locked && preview !== undefined && (reference.kind === 'file' || reference.kind === 'environment_file')) {
    const source = referenceSource(preview);
    if (source !== null) return <SharedFileChip source={{...source,href:target,reference:referenceText(preview)}} reference={reference} presentation={presentation}
      {...(preview.file?.disabled === true ? {label:<>{name} <EnvironmentDot status="disabled" showLabel/></>,ariaLabel:`${name} ${t('environments.status.disabled')}`} : {})} />;
  }
  const hint = locked ? '' : preview?.description.split('\n')[0] ?? '';
  const classes = `${REFERENCE_CHIP_CLASS} ${locked ? 'text-text-muted' : ''}`;
  return <ReferenceCard reference={reference} label={name} presentation={presentation}>{(keep, size) => locked || preview === undefined ?
    <button type="button" aria-label={name} title={name} className={classes} onClick={keep} data-reference-state={deleted ? 'deleted' : 'no-access'}><Icon aria-hidden className="size-4 shrink-0 text-text-muted" /><span className="min-w-0 truncate">{name}</span></button> :
    <a href={target} aria-label={preview.file?.disabled===true?`${name} ${t('environments.status.disabled')}`:name} title={name} draggable onDragStart={(event) => { event.dataTransfer.setData(ORCH_REFERENCE, referenceText(preview)); event.dataTransfer.effectAllowed = 'copy'; }} className={classes} onClick={(event) => { if (isPlainClick(event)) { event.preventDefault(); keep(); } }}>{preview.kind === 'node' && preview.node_kind === 'thread' ? <ThreadStateIcon display={preview.thread_display} className="[&_svg]:size-4" /> : <Icon aria-hidden className="size-4 shrink-0 text-text-muted" />}<span className="min-w-0 truncate">{name}</span>{size==='medium'&&hint!==''&&<span className="min-w-0 truncate text-sm font-normal text-text-muted">{hint}</span>}{preview.file?.disabled===true&&<>{' '}<EnvironmentDot status="disabled" showLabel/></>}</a>
  }</ReferenceCard>;
}

function FileChip({ reference, fallback, presentation }: { reference: TypedReference; fallback: ReactNode; presentation?: ChipPresentation }) {
  return <TypedChip reference={{ ...reference, ...(reference.label !== undefined || typeof fallback !== 'string' ? {} : { label: fallback }) }} presentation={presentation} />;
}

/** A chip for a reference written as `id:<id>#n`, which does not say what it names: its card is asked as a place or a
 * thread, then as a document, then as a chat, and the first that answers is drawn as the chip of that kind. A reader
 * who may not read the thing keeps its authored label, or its kind where no label was stored. A part number opens
 * a thread at its turn and a chat at its message; a document's block is
 * named by its identifier, so a document opens whole. */
export function IdChip({ id, number, fallback, label }: { id: string; number?: number; fallback: ReactNode; label?: string }) {
  const shared = label ?? (typeof fallback === 'string' ? fallback : undefined);
  const named = (reference:TypedReference):TypedReference=>({...reference,...(shared===undefined?{}:{label:shared})});
  const node = useQuery(referencePreviewQuery({ kind: 'node', id }));
  const document = useQuery({ ...referencePreviewQuery({ kind: 'document', id }), enabled: node.isError });
  const chat = useQuery({ ...referencePreviewQuery({ kind: 'chat', id }), enabled: node.isError && document.isError });
  const file = useQuery({ ...referencePreviewQuery({ kind: 'file', id, ...(number === undefined ? {} : { line: number }) }), enabled: node.isError && document.isError && chat.isError });
  if (file.data !== undefined) return <FileChip reference={named({ kind: 'file', id, ...(number === undefined ? {} : { line: number }) })} fallback={fallback} />;
  const found = node.data ?? document.data ?? chat.data;
  if (found === undefined) return file.isError ? <TypedChip reference={named({ kind: 'node', id })} /> : fallback;
  if (found.kind === 'document') return <NamedChip path={`/doc/${id}`} id={id} kind="doc" original={fallback} reference={named({ kind: 'document', id })} />;
  if (found.kind === 'chat') return <NamedChip path={`/chat/${id}`} id={id} kind="chat" original={fallback} reference={named({ kind: 'chat', id, ...(number === undefined ? {} : { message: number }) })} />;
  const thread = found.node_kind === 'thread' || found.node_kind === null;
  return <NamedChip path={`/thread/${id}`} id={id} kind="node" original={fallback} reference={named({ kind: 'node', id, ...(number === undefined || !thread ? {} : { turn: number }) })} />;
}

/** The kind of chip a shared-tree entry is, where it is one: a place, a thread, a conversation, a document or a chat. */
function chipOfEntry(kind: string): Exclude<ChipKind, 'node'> | null {
  switch (kind) {
    case 'org':
    case 'project':
    case 'topic':
    case 'thread':
      return kind;
    case 'conversation':
      return 'thread';
    case 'document':
      return 'doc';
    case 'chat':
      return 'chat';
    default:
      return null;
  }
}

/** The originating thread resolves its own shared-tree paths, with a part after `#`; the caller's projection filters them. */
export function SharedPathChip({ threadId, path, code = false, fallback }: { threadId: string; path: string; code?: boolean; fallback?: ReactNode }) {
  const decoded = (() => { try { return decodeURIComponent(path); } catch { return path; } })();
  // A part is the number after the last `#`, as an agent writes it: a turn of a conversation, a message of a chat.
  const parted = /^(.*)([#:])([1-9]\d*)$/.exec(decoded);
  const bare = parted?.[1] ?? decoded;
  const number = parted?.[3] === undefined ? undefined : Number(parted[3]);
  // A colon names a workspace line; shared names containing one are tried whole.
  const shared = parted?.[2] === ':' ? decoded : bare;
  const sharedNumber = parted?.[2] === ':' ? undefined : number;
  const directory = shared.slice(0, shared.lastIndexOf('/') + 1);
  const parent = directory===''?'.':directory;
  const query = useQuery({ ...sharedTreeQuery(threadId, parent), retry: false, staleTime: 0 });
  const entry = query.isSuccess ? query.data.entries.find((one) => !one.redacted && one.readable && (one.path === shared || one.path.replace(/\/$/, '') === shared.replace(/\/$/, ''))) : undefined;
  const named = entry?.name ?? null;
  const kind = entry === undefined || named === null ? null : chipOfEntry(entry.kind);
  if (entry?.kind === 'file') return <FileChip reference={{ kind: 'file', id: entry.id, ...(sharedNumber === undefined ? {} : { line: sharedNumber }) }} fallback={fallback ?? path} />;
  if (entry === undefined || kind === null) {
    const words = fallback ?? (code ? <code className="rounded-sm bg-bg-muted px-1 font-mono text-sm">{path}</code> : <span>{path}</span>);
    return query.isPending ? words : <FileLink threadId={threadId} path={bare} line={number ?? null}>{words}</FileLink>;
  }
  const reference: TypedReference = kind === 'doc' ? { kind: 'document', id: entry.id } : kind === 'chat' ? { kind: 'chat', id: entry.id, ...(sharedNumber === undefined ? {} : { message: sharedNumber }) } : { kind: 'node', id: entry.id, ...(sharedNumber === undefined || kind !== 'thread' ? {} : { turn: sharedNumber }) };
  return <NamedChip path={kind === 'chat' ? `/chat/${entry.id}` : `/${kind}/${entry.id}`} id={entry.id} kind={kind} original={path} reference={reference} />;
}
