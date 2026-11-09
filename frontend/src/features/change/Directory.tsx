/** A relative working directory uses the ordinary authorized workspace pages, only when opened. */
import { ArrowUp, Folder, FolderOpen } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useQueryClient } from '@tanstack/react-query';
import { environmentKey } from '@/api/environments';
import type { Door } from '@/api/workspace';
import { workspaceBinding } from '@/api/workspaceBinding';
import { useEnvironment } from '@/features/files/door';
import { DirectoryPager } from '@/features/files/DirectoryPager';
import { useDirectoryTree } from '@/features/files/useDirectoryTree';
import { directoryKey } from '@/realtime/stores/environment';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

export function ChangeDirectory({ value, current, door, onChange }: {
  value: string | null; current: string; door: Door | null; onChange: (value: string | null) => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  if (!editing && value === null) return <Button size="sm" variant="ghost" icon={<FolderOpen aria-hidden />} onClick={() => setEditing(true)}>{t('change.directory.change')}</Button>;
  return <div className="flex min-w-0 flex-wrap items-end gap-2" data-change-directory>
    <Input label={t('change.directory.label')} placeholder={t('change.directory.root')} value={value ?? current} onChange={(event) => onChange(event.target.value)} autoComplete="off" fieldClassName="min-w-0 flex-1" />
    <Popover label={t('change.directory.change')} open={browsing} onOpenChange={(open) => setBrowsing(open && door !== null)}
      trigger={<Button size="sm" icon={<FolderOpen aria-hidden />} blocked={door === null} reason={door === null ? t('change.directory.relative') : undefined}>{t('change.directory.browse')}</Button>}>
      {door !== null && <DirectoryPicker key={JSON.stringify(door)} door={door} initial={value ?? current} onChoose={(path) => { onChange(path); setBrowsing(false); }} />}
    </Popover>
    <Button size="sm" variant="ghost" onClick={() => { onChange(null); setEditing(false); setBrowsing(false); }}>{t('change.directory.keep')}</Button>
  </div>;
}

function DirectoryPicker({ door, initial, onChoose }: { door: Door; initial: string; onChoose: (path: string) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const [path, setPath] = useState(initial);
  const { environment, failed } = useEnvironment(door.environmentId);
  const binding = environment === undefined || failed !== null ? null : workspaceBinding(environment);
  const { tree, directories, capacity } = useDirectoryTree(door, path, binding);
  const page = directories.get(directoryKey(door.environmentId, path));
  const parent = path.slice(0, Math.max(0, path.lastIndexOf('/')));
  const ready = failed === null && binding !== null && page?.status === 'ready' && !page.invalidated && !page.restartRequired && page.watch !== 'refused';
  return <div className="flex w-72 max-w-full flex-col gap-2">
    <div className="flex min-w-0 items-center gap-2">
      <IconButton size="sm" label={t('files.pages.openParent')} icon={<ArrowUp aria-hidden />} disabled={path === ''} onClick={() => setPath(parent)} />
      <span className="min-w-0 flex-1 truncate text-sm" title={path || t('change.directory.root')}>{path || t('change.directory.root')}</span>
      {page !== undefined && <DirectoryPager window={page} path={path} navigate={(at, action) => tree.navigate(at, action)} />}
    </div>
    {failed !== null ? <ErrorState compact error={failed} onRetry={() => void cache.refetchQueries({ queryKey: environmentKey(door.environmentId), exact: true })} />
      : capacity !== null || page?.capacity != null ? <StateLine compact title={t('files.pages.unavailable')} action={<Button size="sm" onClick={() => tree.navigate(path, 'restart')}>{t('common.tryAgain')}</Button>} />
      : page !== undefined && (page.status === 'failed' || page.invalidated === true || page.watch === 'refused') ? <ErrorState compact error={page.error} onRetry={page.watch === 'refused' ? undefined : () => tree.navigate(path, page.invalidated === true ? 'restart' : 'retry')} />
      : page === undefined || page.status === 'loading' ? <SkeletonRows rows={3} />
      : <ul className="flex flex-col gap-1" aria-label={t('change.directory.label')}>
        {page.entries.filter((entry) => entry.type === 'dir').map((entry) => <li key={entry.path}>
          <Button variant="ghost" size="sm" icon={<Folder aria-hidden />} className="w-full justify-start" onClick={() => setPath(entry.path)}>{entry.name}</Button>
        </li>)}
        {page.entries.every((entry) => entry.type !== 'dir') && <li><EmptyState compact title={t('change.directory.empty')} /></li>}
      </ul>}
    <Button size="sm" variant="primary" blocked={!ready} onClick={() => { if (ready) onChoose(path); }}>{t('change.directory.use')}</Button>
  </div>;
}
