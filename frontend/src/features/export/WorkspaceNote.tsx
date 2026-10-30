/** The line under the options of a place that says workspace files are not in an archive, and leads to where they are taken away.
 *
 * An archive holds what messages, conversations and documents say and the files they carry; the files an environment's
 * workspace holds are another thing, taken away there: as a snapshot of a directory and its download, or by commit and push.
 * The user guide has no article on it yet, so the link leads to the place the snapshots are: the Environments tab of a
 * topic, and for any other place the list of environments, each of which has its own page. When the guide has the article,
 * the link below is the one thing to change.
 */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import type { Node } from '@/generated/api-types';
import { linkClasses } from '@/ui/link';

import type { ExportTarget } from './target';

/**
 * Whether the line is said. The view of a topic lists the environments its threads run in, so it is said where there is
 * one; the view of an org or a project does not, and what lies below cannot be told without asking for it, so it is said
 * there always.
 */
export function mentionsWorkspace(node: Node): boolean {
  return node.kind !== 'topic' || node.environments.length > 0;
}

export function WorkspaceNote({ target, onClose }: { target: ExportTarget; onClose: () => void }) {
  const { t } = useTranslation();
  const how = t('export.place.workspaceHow');
  return (
    <p className="text-sm text-text-muted">
      {t('export.place.workspaceNote')}{' '}
      {target.kind === 'topic' ? (
        <Link to="/topic/$topicId" params={{ topicId: target.id }} search={{ tab: 'environments' }} onClick={onClose} className={linkClasses}>
          {how}
        </Link>
      ) : (
        <Link to="/resources/environments" onClick={onClose} className={linkClasses}>
          {how}
        </Link>
      )}
    </p>
  );
}
