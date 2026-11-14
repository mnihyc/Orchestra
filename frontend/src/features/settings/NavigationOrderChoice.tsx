/** The order of an org's tree, set from its own page: how places are ordered, and for an order arranged by hand, each group of places in the order they stand, by name.
 *
 * No identifier is shown or typed: a group is named by the place that holds it and each place in it by its own name, moved up or down or taken out; a
 * place that is not arranged yet is added from a list of the places that group holds. A place that no longer exists is shown as such, to be taken out. The raw value
 * stays one press away for whoever wants to write it as it is kept.
 */
import { useQuery } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import { orderOf, type OrderBy, type TreeOrder } from '@/features/navigation/treeOrder';
import { ValueControl, type ValueControlProps } from '@/features/settings/ValueControl';
import type { TreeNode } from '@/generated/api-types';
import { choiceCopy } from '@/i18n/choices';
import { useOrgTreeFeed } from '@/realtime/live';
import { useOrgTree, useTreeNode } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';

export { ORDER_SETTING as NAVIGATION_ORDER_SETTING } from '@/features/navigation/treeOrder';

const BYS: readonly OrderBy[] = ['custom', 'newest', 'changed', 'name'];
type Custom = Readonly<Record<string, readonly string[]>>;

const byName = (first: TreeNode, second: TreeNode) => first.name.localeCompare(second.name);

/** The places of each parent that has any, by name. */
function childrenOf(nodes: ReadonlyMap<string, TreeNode>): Map<string, TreeNode[]> {
  const found = new Map<string, TreeNode[]>();
  for (const row of nodes.values()) {
    if (row.deleted || row.parent_id === null) continue;
    found.set(row.parent_id, [...(found.get(row.parent_id) ?? []), row]);
  }
  for (const rows of found.values()) rows.sort(byName);
  return found;
}

export function NavigationOrderChoice(props: ValueControlProps & { target: SettingsTarget }) {
  const { t } = useTranslation();
  const { value, onChange, onEdited, disabled = false, target } = props;
  const order: TreeOrder = orderOf(value) ?? { by: 'changed' };
  const own = useQuery({ ...nodeQuery(target.kind === 'node' ? target.id : ''), enabled: target.kind === 'node' });
  const root = own.data?.org_id ?? null;
  useOrgTreeFeed(root);
  const tree = useOrgTree(root);
  const [raw, setRaw] = useState(false);
  const [typed, setTyped] = useState('');
  const custom: Custom = order.custom ?? {};
  const set = (next: TreeOrder) => { onEdited?.(); onChange(next.by === 'custom' && next.custom !== undefined && Object.keys(next.custom).length > 0 ? next : { by: next.by }); };
  const arrange = (parent: string, ids: readonly string[]) => {
    const { [parent]: _before, ...rest } = custom;
    set({ by: 'custom', custom: ids.length === 0 ? rest : { ...rest, [parent]: ids } });
  };
  const children = childrenOf(tree.nodes);
  const groups = Object.entries(custom).filter(([, ids]) => ids.length > 0);
  const named = own.data === undefined ? null : { id: own.data.id, name: own.data.name };
  const rooms = [...children.keys()].filter((parent) => custom[parent] === undefined)
    .map((parent) => ({ parent, name: tree.nodes.get(parent)?.name ?? (named?.id === parent ? named.name : '') })).filter((room) => room.name !== '')
    .filter((room) => typed.trim() === '' || room.name.toLocaleLowerCase().includes(typed.trim().toLocaleLowerCase()));
  return <div className="flex min-w-0 flex-col gap-3" data-navigation-order>
    <Select size="sm" label={t('settingsEditors.order.by')} value={order.by} disabled={disabled} fieldClassName="max-w-[20rem]"
      options={BYS.map((by) => ({ value: by, label: choiceCopy(t, `navigationOrder.${by}`).name }))}
      onValueChange={(next) => { const by = BYS.find((candidate) => candidate === next); if (by !== undefined) set(by === 'custom' ? { by, ...(order.custom === undefined ? {} : { custom: order.custom }) } : { by }); }} />
    {order.by === 'custom' && (raw ? <ValueControl {...props} shape={{ type: 'json' }} /> : <>
      {groups.length > 0 && <ul aria-label={t('settingsEditors.order.groups')} className="flex min-w-0 flex-col divide-y divide-border rounded-md border border-border">
        {groups.map(([parent, ids]) => <Group key={parent} parent={parent} ids={ids} fallback={named} disabled={disabled}
          rest={(children.get(parent) ?? []).filter((row) => !ids.includes(row.id))} onChange={(next) => arrange(parent, next)} />)}
      </ul>}
      {root !== null && <Combobox size="sm" label={t('settingsEditors.order.arrange')} query={typed} onQueryChange={setTyped} disabled={disabled || rooms.length === 0 && typed === ''}
        placeholder={t(rooms.length === 0 && typed === '' ? 'settingsEditors.order.nothingToArrange' : 'settingsEditors.order.arrangePlaceholder')}
        groups={[{ id: 'places', label: '', options: rooms.map((room) => ({ value: room.parent, label: room.name, text: room.name })) }]} empty={t('ui.combobox.empty')}
        fieldClassName="max-w-[20rem]" onOpenChange={(open) => { if (!open) setTyped(''); }}
        onSelect={(option) => { setTyped(''); arrange(option.value, (children.get(option.value) ?? []).map((row) => row.id)); }} />}
    </>)}
    {order.by === 'custom' && <div><Button size="sm" variant="ghost" disabled={disabled} onClick={() => setRaw((was) => !was)}>{t(raw ? 'settingsEditors.order.useForm' : 'settingsEditors.order.editJson')}</Button></div>}
  </div>;
}

/** One group: the places of a parent that are arranged, each moved up or down or taken out; the places not yet named are added by name. */
function Group({ parent, ids, rest, fallback, disabled, onChange }: {
  parent: string; ids: readonly string[]; rest: readonly TreeNode[]; fallback: { id: string; name: string } | null; disabled: boolean; onChange: (ids: readonly string[]) => void;
}) {
  const { t } = useTranslation();
  const place = useTreeNode(parent);
  const name = place?.name ?? (fallback?.id === parent ? fallback.name : t('settingsEditors.order.gone'));
  const [typed, setTyped] = useState('');
  const match = typed.trim().toLocaleLowerCase();
  const move = (at: number, by: number) => {
    const next = [...ids];
    const held = next[at];
    const other = next[at + by];
    if (held === undefined || other === undefined) return;
    next[at] = other;
    next[at + by] = held;
    onChange(next);
  };
  return <li data-order-group={parent} className="flex min-w-0 flex-col gap-2 px-3 py-2">
    <div className="flex min-w-0 items-center justify-between gap-2">
      <span className="min-w-0 font-medium wrap-anywhere">{name}</span>
      <IconButton size="sm" label={t('settingsEditors.order.removeGroup', { name })} icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange([])} />
    </div>
    <ol className="flex min-w-0 flex-col gap-1">
      {ids.map((id, at) => <Place key={id} id={id} name={name} first={at === 0} last={at === ids.length - 1} disabled={disabled}
        onMove={(by) => move(at, by)} onRemove={() => onChange(ids.filter((other) => other !== id))} />)}
    </ol>
    {rest.length > 0 && <Combobox size="sm" label={t('settingsEditors.order.add', { name })} query={typed} onQueryChange={setTyped} disabled={disabled} fieldClassName="max-w-[20rem]"
      placeholder={t('settingsEditors.order.addPlaceholder')} empty={t('ui.combobox.empty')} onOpenChange={(open) => { if (!open) setTyped(''); }}
      groups={[{ id: 'places', label: '', options: rest.filter((row) => match === '' || row.name.toLocaleLowerCase().includes(match)).map((row) => ({ value: row.id, label: row.name, text: row.name })) }]}
      onSelect={(option) => { setTyped(''); onChange([...ids, option.value]); }} />}
  </li>;
}

function Place({ id, name: group, first, last, disabled, onMove, onRemove }: {
  id: string; name: string; first: boolean; last: boolean; disabled: boolean; onMove: (by: number) => void; onRemove: () => void;
}) {
  const { t } = useTranslation();
  const place = useTreeNode(id);
  const name = place?.name ?? t('settingsEditors.order.gone');
  return <li data-order-place={id} className="flex min-w-0 items-center justify-between gap-2">
    <span className={place === null ? 'min-w-0 text-text-muted wrap-anywhere' : 'min-w-0 wrap-anywhere'}>{name}</span>
    <span className="flex shrink-0 items-center gap-1">
      <IconButton size="sm" label={t('settingsEditors.order.up', { name, group })} icon={<ArrowUp aria-hidden />} disabled={disabled || first} onClick={() => onMove(-1)} />
      <IconButton size="sm" label={t('settingsEditors.order.down', { name, group })} icon={<ArrowDown aria-hidden />} disabled={disabled || last} onClick={() => onMove(1)} />
      <IconButton size="sm" label={t('settingsEditors.order.remove', { name, group })} icon={<X aria-hidden />} disabled={disabled} onClick={onRemove} />
    </span>
  </li>;
}
