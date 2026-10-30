/** A project's Activity: the lines of its own history and of the management of its topics.
 *
 * Opened from the project's views: the lines of what was done to the project, to its topics' management,
 * and what its threads did, with the chips that keep to one of the two. Each line says who did it, and a
 * line about a topic or a thread opens it. It takes in the folders of the project, and says so.
 */
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { subtreeCounts } from '@/features/navigation/places';
import { ActivityFeed } from '@/features/org/Activity';
import { useOrgTree } from '@/realtime/stores/tree';

export function ProjectActivity({ projectId, name, orgId }: { projectId: string; name: string; orgId: string }) {
  const { t } = useTranslation();
  const tree = useOrgTree(orgId);
  const folders = useMemo(() => subtreeCounts(tree.nodes, projectId).folders, [tree.nodes, projectId]);
  return (
    <div className="flex flex-col gap-3">
      {folders > 0 && <p className="text-sm text-text-muted">{t('folders.takesIn', { name, count: folders })}</p>}
      <ActivityFeed nodeId={projectId} chips />
    </div>
  );
}
