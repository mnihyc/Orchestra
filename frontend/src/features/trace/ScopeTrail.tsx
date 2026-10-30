/** A single node's trail where the node has no settings leaf, with its own audit request deciding access. */
import { useQuery } from '@tanstack/react-query';
import { ScrollText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { nodeQuery } from '@/api/nodes';
import { Page, PageHeader } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import type { TraceScope } from './params';
import { scopeOf } from './params';
import { Trail } from './Trail';

export function ScopeTrail({ id }: { id: string }) {
  const { t } = useTranslation(); const node = useQuery({ ...nodeQuery(id), staleTime: 0 });
  const scope: TraceScope = node.data === undefined ? { type: 'node', id, name: t('trace.unknown'), orgId: id, kind: 'thread', access: undefined } : scopeOf(node.data);
  useScreenHeading({ title: t('settings.org.sectionNames.audit'), icon: ScrollText, ...(node.data === undefined ? {} : { nodeId: node.data.id, path: node.data.path, leaf: true }) });
  return <Page><PageHeader title={t('trace.title', { scope: scope.name })} /><Trail key={id} scope={scope} embedded /></Page>;
}
