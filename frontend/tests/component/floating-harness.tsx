/** Real shared layers with deliberately long contents and edge anchors, served only by the test development server. */
import { QueryClientProvider } from '@tanstack/react-query';
import { RouterContextProvider } from '@tanstack/react-router';
import { useState, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { router } from '@/app';
import { queryClient } from '@/api/query';
import { referencePreviewQuery } from '@/api/references';
import { SuggestMenu, useSuggest } from '@/features/documents/BlockMenu';
import { ReaderComment } from '@/features/people/ReaderComment';
import type { ReferencePreview } from '@/generated/api-types';
import { AgentCard } from '@/ui/AgentCard';
import { AvatarStack } from '@/ui/AvatarStack';
import { Combobox } from '@/ui/Combobox';
import { ContextMenu } from '@/ui/ContextMenu';
import { DateField } from '@/ui/DateField';
import { DetailCard } from '@/ui/DetailCard';
import { Dialog } from '@/ui/Dialog';
import { HelpHint } from '@/ui/HelpHint';
import { HolderChip, type PeopleSource } from '@/ui/HolderChip';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { PersonCard } from '@/ui/PersonCard';
import { Popover, floatingClasses, floatingPoint } from '@/ui/Popover';
import { ReferenceCard } from '@/ui/ReferenceCard';
import { Select } from '@/ui/Select';
import { Tooltip, TooltipProvider } from '@/ui/Tooltip';
import { me, ACME } from '../doubles/fixtures';
import { i18n } from '@/i18n';

import type { FloatingKind } from '../e2e/floating-cases';
let root: Root | undefined;
let host: HTMLDivElement | undefined;
const ada = me(1, 'ada', 'Ada Lovelace');
const reference = { kind: 'node', id: ACME.fixLogin } as const;

function Example({ kind, left, top, text }: { kind: FloatingKind; left: number; top: number; text: string }) {
  const [query, setQuery] = useState('');
  const [value, setValue] = useState('1');
  const trigger = <button type="button" data-floating-trigger>Open layer</button>;
  const long = <div className="w-full wrap-anywhere">{Array.from({ length: 90 }, (_, i) => <p key={i}>{text} {i + 1}</p>)}</div>;
  const entries: MenuEntry[] = Array.from({ length: 80 }, (_, i) => ({ id: String(i), label: `${text} ${i + 1}`, onSelect: () => undefined }));
  const options = Array.from({ length: 80 }, (_, i) => ({ value: String(i), label: `${text} ${i + 1}`, text: `${text} ${i + 1}` }));
  const people: PeopleSource = { queryKey: ['floating-holder'], initialPageParam: null, getNextPageParam: () => null,
    queryFn: () => Promise.resolve({ items: Array.from({ length: 80 }, (_, i) => ({ ...ada, id: String(i) })), next_cursor: null }) };
  let content: ReactNode;
  switch (kind) {
    case 'help': content = <HelpHint term="Layer" what={long} />; break;
    case 'popover': content = <Popover label="Layer" trigger={trigger}>{long}</Popover>; break;
    case 'detail': content = <DetailCard label="Layer" content={() => long} asChild>{trigger}</DetailCard>; break;
    case 'person': content = <PersonCard user={ada} actions={long}>Open layer</PersonCard>; break;
    case 'agent': content = <AgentCard name={text.repeat(80)}>Open layer</AgentCard>; break;
    case 'reference': content = <ReferenceCard reference={reference} label="Layer">{(keep) => <button type="button" onClick={keep}>Open layer</button>}</ReferenceCard>; break;
    case 'tooltip': content = <Tooltip content={long}>{trigger}</Tooltip>; break;
    case 'menu': content = <Menu label="Layer" trigger={trigger} entries={entries} />; break;
    case 'submenu': content = <Menu label="Layer" trigger={trigger} entries={[{ type: 'sub', id: 'nested', label: 'Nested layer', entries }, entries[0]!]} />; break;
    case 'context': content = <ContextMenu label="Layer" entries={entries}>{trigger}</ContextMenu>; break;
    case 'select': content = <Select label="Layer" hideLabel value={value} onValueChange={setValue} options={options} />; break;
    case 'combobox':
    case 'paged-combobox': content = <Combobox label="Layer" hideLabel query={query} onQueryChange={setQuery} groups={[{ id: 'choices', label: '', options }]} onSelect={() => undefined}
      footer={kind === 'paged-combobox' ? <button type="button">Next page</button> : undefined} />; break;
    case 'date': content = <DateField label="Layer" hideLabel value="2026-10-25" onValueChange={() => undefined} />; break;
    case 'avatars': content = <AvatarStack users={Array.from({ length: 80 }, (_, i) => ({ ...ada, id: String(i), display_name: `${text} ${i + 1}` }))} />; break;
    case 'holder': content = <HolderChip name={text.repeat(80)} people={people} />; break;
    case 'suggestions': content = <SuggestMenu />; break;
    case 'reader': content = <><div data-document="floating-document"><p data-id="floating-block">Selected words</p></div><ReaderComment docId="floating-document" /></>; break;
    case 'overflow': content = <span className="block truncate">{text.repeat(300)}</span>; break;
    case 'fixed': content = <div data-fixed-layer className={`fixed overflow-y-auto ${floatingClasses}`} style={floatingPoint(left, top)}>{long}</div>; break;
  }
  return <div data-floating-example style={{ position: 'fixed', left, top, width: kind === 'select' || kind.includes('combobox') ? 800 : 150 }}>{content}</div>;
}

export async function mountFloating(kind: FloatingKind, left: number, top: number, text: string): Promise<void> {
  flushSync(() => root?.unmount());
  host?.remove();
  // Let the previous primitive return focus before installing the next independent example.
  await new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
  useSuggest.setState({ open: false });
  document.getSelection()?.removeAllRanges();
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  const preview: ReferencePreview = { ...reference, node_kind: 'topic', title: text.repeat(80), description: '', place: null, part: null, updated_at: null, updated_by: null };
  queryClient.setQueryData(referencePreviewQuery(reference).queryKey, preview);
  flushSync(() => root!.render(<QueryClientProvider client={queryClient}><RouterContextProvider router={router}><TooltipProvider><Example kind={kind} left={left} top={top} text={text} /></TooltipProvider></RouterContextProvider></QueryClientProvider>));
  if (kind === 'suggestions') useSuggest.setState({ open: true, kind: 'block', index: 0, rect: { left, top, bottom: top + 20 }, items: Array.from({ length: 80 }, (_, i) => ({ id: String(i), label: `${text} ${i + 1}` })) });
  if (kind === 'reader') {
    const range = document.createRange(); range.selectNodeContents(host.querySelector('[data-id]')!);
    document.getSelection()?.addRange(range); document.dispatchEvent(new Event('selectionchange'));
  }
}

export async function setFloatingAppearance(theme: string, locale: string): Promise<void> {
  await i18n.changeLanguage(locale);
  document.documentElement.dataset.theme = theme; document.documentElement.lang = locale;
}

function LayeredOverflow({ text }: { text: string }) {
  const [open, setOpen] = useState(true);
  return <Dialog open={open} onOpenChange={setOpen} title="Layer parent" size="sm">
    <DetailCard label={text} content={() => <p>Inspection details</p>}>
      <span className="block truncate" style={{ width: 80 }}>{text}</span>
    </DetailCard>
  </Dialog>;
}

export async function mountLayeredOverflow(text: string): Promise<void> {
  flushSync(() => root?.unmount());
  host?.remove();
  await new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done())));
  host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  flushSync(() => root!.render(<QueryClientProvider client={queryClient}><RouterContextProvider router={router}><TooltipProvider><LayeredOverflow text={text} /></TooltipProvider></RouterContextProvider></QueryClientProvider>));
}
