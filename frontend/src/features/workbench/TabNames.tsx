/** Name unopened content tabs from already authorized navigation rows, without mounting their readers. */
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { Box } from 'lucide-react';
import { THREAD_GLYPHS } from '@/ui/ThreadState';
import { NODE_GLYPHS } from '@/ui/NodeIcon';
import { placedOfRow } from '@/features/navigation/placeLabels';
import { knownPlaces, placeLabels, type KnownPlaces } from '@/lib/placeLabel';
import { pathTo, useTreeStore, type OrgTree } from '@/realtime/stores/tree';
import type { TreeNode } from '@/generated/api-types';
import { allTabs, type Layout, type Tab } from './layout';
import { pageAddress, pageResource, parsePageAddress } from './PageView';
import { useWorkbench } from './store';
import { staticPageName } from './pageNames';
import { useViews } from './views';
import { tabCaptions } from './tabCaptions';
import { formatNumber } from '@/lib/format';

/** Existing names, readable paths and public kind labels only; no hidden tab reader is mounted. */
export function useTabCaptions(tabs: readonly Tab[]): Map<string, string> {
  const { t, i18n } = useTranslation();
  const named = useWorkbench((state) => state.named);
  const definitions = useViews();
  return tabCaptions(tabs.map((tab) => {
    const definition = definitions.find((view) => view.name === tab.view), held = named.get(tab.id);
    const baseTitle = held?.title ?? (definition === undefined ? tab.view : t(definition.title));
    const href = tab.view === 'page' ? pageAddress(tab) : null, page = href === null ? null : parsePageAddress(href);
    const management = t('admin.nav.label');
    const title = page !== null && (page.pathname === '/admin' || page.pathname.startsWith('/admin/')) && baseTitle !== management ? management + ' · ' + baseTitle : baseTitle;
    const resource = pageResource(page);
    const category = page?.pathname.match(/^\/resources\/[^/]+/)?.[0];
    const kind = resource !== null ? t(`everything.kinds.${resource.kind}`) : category !== undefined && page !== null
      ? staticPageName({ ...page, pathname: category }, t)?.title : tab.view !== 'page' && definition !== undefined ? t(definition.title) : undefined;
    return { id: tab.id, title, ...(kind === undefined ? {} : { kind }), ...(held?.path === undefined ? {} : { path: held.path }) };
  }), (name, number) => t('workbench.tabInstance', { name, number: formatNumber(number, i18n.resolvedLanguage ?? 'en') }));
}

export function CachedTabNames({ layout }: { layout: Layout }) {
  const { t } = useTranslation();
  const trees = useTreeStore((state) => state.byOrg);
  useEffect(() => {
    const before = useWorkbench.getState().named;
    const next = new Map(before);
    let known: KnownPlaces | undefined;
    for (const tab of allTabs(layout)) {
      if (tab.view !== 'page' || before.get(tab.id)?.source === 'view') continue;
      const href = pageAddress(tab);
      const page = href === null ? null : parsePageAddress(href);
      const plain = page === null ? null : staticPageName(page, t);
      if (plain !== null) {
        const held = before.get(tab.id);
        if (held?.title !== plain.title || held.icon !== plain.icon || held.path !== undefined || held.source !== 'cache') {
          next.set(tab.id, { ...plain, source: 'cache' });
        }
        continue;
      }
      const target = pageResource(page);
      if (target === null) continue;
      let found: { tree: OrgTree; node: TreeNode } | undefined;
      for (const tree of trees.values()) {
        const node = tree.nodes.get(target.id);
        if (node !== undefined && !node.deleted) { found = { tree, node }; break; }
      }
      let title = before.get(tab.id)?.title ?? t(`everything.kinds.${target.kind}`);
      if (found !== undefined) {
        // Only an unopened native tab needs a tree label. Live titles and static pages do not
        // qualify thousands of unrelated nodes whenever a thread's running state changes.
        known ??= knownPlaces([...trees.values()].flatMap((tree) => [...tree.nodes.values()].filter((node) => !node.deleted).map((node) => placedOfRow(tree, node))));
        title = placeLabels([placedOfRow(found.tree, found.node)], known).get(target.id) ?? found.node.name;
      }
      const path = found === undefined ? before.get(tab.id)?.path : pathTo(found.tree, found.node.id).map((node) => node.name).join(' / ');
      const display = found?.node.thread?.display;
      const icon = target.kind === 'thread' && display !== undefined ? THREAD_GLYPHS[display.state] : target.kind === 'environment' ? Box : NODE_GLYPHS[target.kind];
      if (before.get(tab.id)?.title !== title || before.get(tab.id)?.path !== path || before.get(tab.id)?.icon !== icon) next.set(tab.id, { title, icon, path, source: 'cache', ...(target.kind === 'thread' && display !== undefined ? { thread: { id: target.id, display } } : {}) });
    }
    if ([...next].some(([id, name]) => before.get(id) !== name)) useWorkbench.setState({ named: next });
  }, [layout, trees, t]);
  return null;
}
