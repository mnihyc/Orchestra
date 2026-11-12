/** Files and shells opened from an environment's own page with its own permissions. */
import { lazy, Suspense, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Folder, SquareTerminal } from 'lucide-react';
import { environmentWorkspaceAccess, workspaceDenial } from '@/features/files/door';
import type { Environment } from '@/generated/api-types';
import { EnvironmentSnapshots } from './Snapshots';
import { EmptyState } from '@/ui/EmptyState';
import { SkeletonRows } from '@/ui/Skeleton';

const Files = lazy(() => import('@/features/inspector/Files').then((part) => ({ default: part.FilesPanel })));
const Terminal = lazy(() => import('@/features/inspector/Terminal').then((part) => ({ default: part.TerminalPanel })));

export function EnvironmentDoor({ environment, kind }: { environment: Environment; kind: 'files' | 'terminal' }) {
  const { t } = useTranslation();
  const access = useMemo(() => environmentWorkspaceAccess(environment), [environment]);
  const permission = kind === 'files' ? 'files.view' : 'terminal.manage';
  const admitted = kind === 'files' ? access.mayView : access.mayShell;
  if (!admitted) return <EmptyState compact icon={kind === 'files' ? <Folder aria-hidden /> : <SquareTerminal aria-hidden />} title={workspaceDenial(t, permission, environment)} />;
  return <div className="flex h-full min-h-0 flex-col gap-3" data-testid={`environment-${kind}`}><div className="min-h-0 flex-1 overflow-hidden rounded-lg border border-border"><Suspense fallback={<SkeletonRows rows={6} />}>{kind === 'files' ? <Files key={environment.id} access={access} /> : <Terminal key={environment.id} access={access} />}</Suspense></div>{kind === 'files' && <EnvironmentSnapshots key={environment.id} id={environment.id} />}</div>;
}
