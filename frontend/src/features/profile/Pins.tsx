/** An ordered list of visible pins, chosen from favourites, recents and authorized name search. */
import { useIsMutating, useQuery, useQueryClient } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { favoritesQuery } from '@/api/personal';
import { profileKey, replaceProfilePins } from '@/api/profile';
import { namesQuery } from '@/api/search';
import { AutoSaveFeedback, useAutoSave } from '@/features/settings/AutoSave';
import type { NodeRef, ProfilePinInput, UserProfile } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import type { Placed } from '@/lib/placeLabel';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { IconButton } from '@/ui/IconButton';

const PIN_LIMIT = 6; // The page and its wire contract permit six ordered pins.
interface Choice { key: string; title: string; pin: ProfilePinInput; place: Placed }
const placed = (id: string, name: string, path: readonly NodeRef[]): Placed => ({ id, name, above: path.filter((one) => one.id !== id).map((one) => ({ id: one.id, name: one.redacted ? null : one.name })) });
const fromProfile = (profile: UserProfile): Choice[] => profile.pins.flatMap((pin): Choice[] => pin.node !== null ? [{ key: `node:${pin.node.id}`, title: pin.node.name, pin: { node_id: pin.node.id }, place: { ...placed(pin.node.id, pin.node.name, pin.node.path), ...(pin.node.kind === 'org' ? { address: pin.node.address } : {}) } }] : pin.document !== null ? [{ key: `document:${pin.document.id}`, title: pin.document.title, pin: { document_id: pin.document.id }, place: placed(pin.document.id, pin.document.title, [pin.document.node]) }] : []);

export function PinsEditor({ profile }: { profile: UserProfile }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const busy = useIsMutating({ mutationKey: profileKey(profile.user.username), exact: true }) > 0;
  const save = useAutoSave({ current: { value: fromProfile(profile) }, blocked: busy, mutationKey: profileKey(profile.user.username),
    write: async (selected) => {
      const accepted = await replaceProfilePins(selected.map((choice) => choice.pin));
      queryClient.setQueryData(profileKey(accepted.user.username), accepted); return { value: fromProfile(accepted) };
    } });
  const pins = save.value;
  const setPins = save.choose;
  const [text, setText] = useState('');
  const term = text.trim();
  const rested = useDebounced(term);
  const [announcement, announce] = useState('');
  const dragged = useRef<number | null>(null);
  const favorite = useQuery(favoritesQuery);
  const names = useQuery({ ...namesQuery({ q: rested, kinds: ['org', 'project', 'topic', 'thread', 'document'] }), enabled: term === rested });
  const current = term === rested && !names.isPlaceholderData ? names.data ?? [] : [];
  const labels = usePlaceLabels([
    ...pins.map((pin) => pin.place),
    ...(favorite.data ?? []).flatMap((entry) => entry.node.name === null || entry.node.redacted ? [] : [placed(entry.node.id, entry.node.name, entry.path)]),
    ...current.map((hit) => ({ ...placed(hit.id, hit.name, hit.path), ...(hit.address === null ? {} : { address: hit.address }) })),
  ], { trees: false });
  const titleOf = (choice: Choice) => labels.get(choice.place.id) ?? choice.title;
  const lookupError = term === rested ? names.error ?? (term === '' ? favorite.error : null) : null;
  const candidates = new Map<string, Choice>();
  for (const entry of favorite.data ?? []) if (!entry.node.redacted && entry.node.name !== null && (term === '' || entry.node.name.toLocaleLowerCase().includes(term.toLocaleLowerCase()))) candidates.set(`node:${entry.node.id}`, { key: `node:${entry.node.id}`, title: entry.node.name, pin: { node_id: entry.node.id }, place: placed(entry.node.id, entry.node.name, entry.path) });
  for (const hit of current) {
    const document = hit.kind === 'document';
    const key = `${document ? 'document' : 'node'}:${hit.id}`;
    candidates.set(key, { key, title: hit.name, pin: document ? { document_id: hit.id } : { node_id: hit.id }, place: { ...placed(hit.id, hit.name, hit.path), address: hit.address } });
  }
  const add = (choice: Choice) => { setPins((current) => current.length >= PIN_LIMIT || current.some((pin) => pin.key === choice.key) ? current : [...current, choice]); announce(''); };
  const remove = (key: string) => { setPins((current) => current.filter((pin) => pin.key !== key)); announce(''); };
  const move = (from: number, to: number) => {
    if (busy || to < 0 || to >= pins.length || from === to) return;
    const next = [...pins]; const [pin] = next.splice(from, 1); if (pin === undefined) return;
    next.splice(to, 0, pin); setPins(next); announce(t('profile.page.moved', { name: titleOf(pin), position: to + 1 }));
  };
  return <section className="flex flex-col gap-3" aria-label={t('profile.page.pinned')}><h2 className="text-lg font-semibold">{t('profile.page.pinned')}</h2><p className="text-sm text-text-muted">{t('profile.page.pinHelp')}</p>
    <ol className="flex flex-col gap-2">{pins.map((pin, index) => <li key={pin.key} draggable={!busy} onDragStart={() => { dragged.current = index; }} onDragEnd={() => { dragged.current = null; }} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); if (dragged.current !== null) move(dragged.current, index); dragged.current = null; }} className="flex flex-wrap items-start gap-2 rounded-md border border-border p-2"><span className="min-w-0 flex-1 text-sm leading-(--control-sm) wrap-anywhere">{titleOf(pin)}</span><IconButton size="sm" action="earlier" disabled={index === 0 || busy} label={t('profile.page.moveUp', { name: titleOf(pin) })} onClick={() => move(index, index - 1)} /><IconButton size="sm" action="later" disabled={index === pins.length - 1 || busy} label={t('profile.page.moveDown', { name: titleOf(pin) })} onClick={() => move(index, index + 1)} /><Button size="sm" disabled={busy} aria-label={t('profile.page.removePin', { name: titleOf(pin) })} onClick={() => remove(pin.key)}>{t('profile.page.remove')}</Button></li>)}</ol>
    <Input label={t('profile.page.findPin')} value={text} onChange={(event) => setText(event.target.value)} />
    {lookupError !== null && <ErrorState compact error={lookupError} retrying={names.isRefetching || favorite.isRefetching} onRetry={() => { if (names.isError) void names.refetch(); else void favorite.refetch(); }} />}
    <ul className="flex max-h-56 flex-col gap-1 overflow-auto" aria-busy={term !== rested || names.isFetching}>{[...candidates.values()].filter((candidate) => !pins.some((pin) => pin.key === candidate.key)).map((choice) => <li key={choice.key} data-pin-choice={choice.key}><Button size="sm" disabled={pins.length >= PIN_LIMIT || busy || names.isPlaceholderData} onClick={() => add(choice)}>{titleOf(choice)}</Button></li>)}</ul>
    <AutoSaveFeedback save={save} name={t('profile.page.pinned')} /><p role="status" className="sr-only">{announcement}</p>
  </section>;
}
