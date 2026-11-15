/** A custom navigation order is read by the names of its places, including when its setting cannot be edited. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import { orderOf } from '@/features/navigation/treeOrder';
import { useOrgTreeFeed } from '@/realtime/live';
import { useOrgTree } from '@/realtime/stores/tree';
import { choiceCopy } from '@/i18n/choices';
import { Input } from '@/ui/Input';

export function NavigationOrderSummary({ value, target }: { value: unknown; target?: SettingsTarget | undefined }) {
  const { t } = useTranslation();
  const order = orderOf(value);
  const own = useQuery({ ...nodeQuery(target?.kind === 'node' ? target.id : ''), enabled: target?.kind === 'node' && order?.by === 'custom', retry: false });
  const root = own.data?.org_id ?? null;
  useOrgTreeFeed(order?.by === 'custom' ? root : null);
  const tree = useOrgTree(root);
  const [query, setQuery] = useState('');
  const name = (id: string): string => {
    const place = tree.nodes.get(id);
    if (place !== undefined) return place.deleted ? t('settingsEditors.order.gone') : place.name;
    if (tree.hidden?.has(id) === true) return t('access.place.hidden');
    if (own.data?.id === id) return own.data.name;
    if (tree.loaded) return t('settingsEditors.order.gone');
    return t(own.isError || tree.error !== null ? 'settings.resourceChoice.unavailable' : target === undefined ? 'access.place.hidden' : 'common.loading');
  };
  const needle = query.trim().toLocaleLowerCase();
  const groups = Object.entries(order?.by === 'custom' ? order.custom ?? {} : {}).map(([parent, ids]) => ({ parent, title: name(parent), places: ids.map((id) => ({ id, title: name(id) })) }));
  const long = groups.length > 10 || groups.some((group) => group.places.length > 10);
  const shown = groups.map((group) => ({ ...group, places: group.title.toLocaleLowerCase().includes(needle) ? group.places : group.places.filter((place) => place.title.toLocaleLowerCase().includes(needle)) }))
    .filter((group) => needle === '' || group.title.toLocaleLowerCase().includes(needle) || group.places.length > 0);
  return <div className="flex min-w-0 flex-col gap-2" data-navigation-order-summary>
    <span>{order === null ? t('settings.compact.none') : choiceCopy(t, `navigationOrder.${order.by}`).name}</span>
    {long && <Input type="search" hideLabel label={t('settingsEditors.order.search')} placeholder={t('settingsEditors.order.search')} value={query} onChange={(event) => setQuery(event.target.value)} />}
    {groups.length > 0 && <ul aria-label={t('settingsEditors.order.groups')} className="flex max-h-80 min-w-0 flex-col gap-2 overflow-y-auto overscroll-contain">
      {shown.map((group) => <li key={group.parent} className="min-w-0">
        <span className="font-medium wrap-anywhere">{group.title}</span>
        <ol className="ml-4 list-decimal pl-4">{group.places.map((place) => <li key={place.id} className="wrap-anywhere">{place.title}</li>)}</ol>
      </li>)}
    </ul>}
    {groups.length > 0 && shown.length === 0 && <p role="status" className="text-text-muted">{t('ui.combobox.empty')}</p>}
  </div>;
}
