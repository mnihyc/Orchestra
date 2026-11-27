/** One root and short-subdirectory control for machine registration and a thread's workspace. */
import { PathLink } from '@/shell/TrailLink';
import { useQuery } from '@tanstack/react-query';
import { ArrowUp, Folder, FolderOpen } from 'lucide-react';
import { useEffect, useImperativeHandle, useState, type ReactNode, type Ref } from 'react';
import { ErrorState } from '@/ui/ErrorState';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { ApiError, client } from '@/api/client';
import { machineEnvironmentDataQuery } from '@/api/resource-tools';
import { listMachineDirectories } from '@/api/directories';
import { useViewChoice } from '@/lib/viewChoices';
import { Button } from '@/ui/Button';
import { Combobox } from '@/ui/Combobox';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { SkeletonRows } from '@/ui/Skeleton';
import { DirectoryListing, directoryReader, directoryScope, type DirectoryLevel, type DirectoryTarget } from './listing';

export interface DirectoryControlHandle { verify: () => Promise<void> }
interface Props {
  ref?: Ref<DirectoryControlHandle>;
  target: DirectoryTarget; value: string; onChange: (value: string) => void;
  error?: unknown; disabled?: boolean; onReady?: (ready: boolean) => void;
}
function strings(raw: string): string[] {
  try { const parsed: unknown = JSON.parse(raw); return Array.isArray(parsed) ? parsed.filter((value): value is string => typeof value === 'string') : []; }
  catch { return []; }
}
const trimEnd = (value: string): string => value.replace(/\/+$/u, '');
const separator = (root: string): string => root.includes('\\') ? '\\' : '/';
function absolute(root: string, path: string): string {
  const join = separator(root), base = root.replace(/[\\/]+$/u, '');
  return path === '' ? root : `${base}${join}${join === '\\' ? path.replace(/\//gu, '\\') : path}`;
}
function relative(value: string, root: string): string {
  if (value === root || value === '' || root === '') return '';
  const base = absolute(root, ''), join = separator(root), prefix = `${base.replace(/[\\/]+$/u, '')}${join}`;
  return value.startsWith(prefix) ? value.slice(prefix.length).split(join).join('/') : '';
}
function childrenOf(kept: readonly string[], target: DirectoryTarget, root: string, parent: string): string[] {
  const paths = kept.map((path) => target.kind === 'machine' ? relative(path, root) : path);
  const prefix = parent === '' ? '' : `${parent}/`;
  return [...new Set(paths.flatMap((path) => path.startsWith(prefix) ? [path.slice(prefix.length).split('/')[0] ?? ''].filter(Boolean) : []))];
}
export function directoryProblem(error: unknown): string | undefined {
  if (error === null || error === undefined) return undefined;
  if (error instanceof ApiError) {
    const native = error.extra('request.invalid')?.errors.find((fault) => (fault.path === 'path' || fault.path === 'root') && fault.message.startsWith('environment.roots:'));
    return native?.message ?? error.faults().find((fault) => fault.path === 'path' || fault.path === 'root')?.message ?? error.message;
  }
  return error instanceof Error ? error.message : typeof error === 'string' ? error : undefined;
}

export function DirectoryControl(props: Props) {
  const reader = useStore(client.session, () => directoryReader());
  return <DirectorySession key={JSON.stringify([reader, directoryScope(props.target)])} {...props} />;
}

function DirectorySession({ target, value, onChange, error, disabled = false, onReady, ref }: Props) {
  const { t } = useTranslation();
  const user = useStore(client.session, (state) => state.userId);
  const machine = target.kind === 'machine' ? target.machine : null;
  const roots = useQuery({ ...machineEnvironmentDataQuery(machine?.id ?? '', directoryReader()), enabled: machine !== null, retry: false, refetchOnMount: 'always' });
  const [root, setRoot] = useState('');
  const [rootSearch, searchRoot] = useState('');
  const [keptRoots, keepRoots] = useViewChoice<string>('directory', 'roots', '[]', directoryScope(target));
  const [keptPaths, keepPaths] = useViewChoice<string>('directory', 'paths', '[]', directoryScope(target));
  const recentRoots = strings(keptRoots), recentPaths = strings(keptPaths);
  const offered = roots.isSuccess && !roots.isFetching ? roots.data.roots.map((root) => root.path) : [];
  offered.sort((left, right) => { const l = recentRoots.indexOf(left), r = recentRoots.indexOf(right); return (l < 0 ? Infinity : l) - (r < 0 ? Infinity : r); });
  const selected = machine === null || offered.includes(root);
  const rootReady = machine === null || roots.isSuccess && !roots.isFetching && selected && root !== '';
  if (machine !== null && root === '' && roots.isSuccess && !roots.isFetching) {
    const matched = offered.filter((path) => value === path || relative(value, path) !== '').sort((a, b) => b.length - a.length)[0];
    const next = matched ?? (value === '' ? offered[0] : undefined);
    if (next !== undefined) setRoot(next);
  }
  useEffect(() => { if (machine !== null && rootReady && value === '') onChange(root); }, [machine, rootReady, root, value, onChange]);
  const decoded = machine === null ? value : relative(value, root);
  const [draft, setDraft] = useState({ external: value, root, text: decoded });
  if (draft.external !== value || draft.root !== root) {
    const serialized = machine === null ? trimEnd(draft.text) : absolute(root, trimEnd(draft.text));
    setDraft({ external: value, root, text: serialized === value ? draft.text : decoded });
  }
  const [opened, setOpened] = useState(false);
  const [browsing, setBrowsing] = useState(false);
  const source = machine !== null ? rootReady : target.kind === 'environment' && target.door !== null;
  const text = draft.root === root && draft.external === value ? draft.text : decoded;
  const cut = text.lastIndexOf('/'), parent = cut < 0 ? '' : text.slice(0, cut), prefix = text.slice(cut + 1);
  const recent = childrenOf(recentPaths, target, root, parent);
  useImperativeHandle(ref, () => ({ verify: async () => {
    if (machine === null) return;
    if (!rootReady) throw new Error(t('directoryChoice.missingRoot'));
    const checked = await listMachineDirectories(machine.id, { read_kind: 'inspect', root, path: trimEnd(text), prefix: '', recent: null, cursor: null, limit: 1 });
    if (checked.read_kind !== 'inspect') throw new Error(t('directoryChoice.readFailed'));
  } }), [machine, rootReady, root, text, t]);
  const write = (text: string, base = root) => {
    const next = machine === null ? trimEnd(text) : absolute(base, trimEnd(text));
    if (machine !== null) setRoot(base);
    setDraft({ external: next, root: base, text }); onChange(next);
  };
  const remember = (path: string, base = root) => {
    const next = machine === null ? path : absolute(base, path);
    keepPaths(JSON.stringify([next, ...recentPaths.filter((old) => old !== next)]));
    if (machine !== null) keepRoots(JSON.stringify([base, ...recentRoots.filter((old) => old !== base)]));
  };
  const choose = (path: string) => { write(path === '' ? '' : `${path}/`); remember(path); };
  const emptyRoots = machine !== null && roots.isSuccess && !roots.isFetching && offered.length === 0;
  const rootError = machine === null ? undefined : roots.isError ? directoryProblem(roots.error) : emptyRoots ? t('directoryChoice.noRoots') : root !== '' && !selected && !roots.isFetching ? t('directoryChoice.missingRoot') : undefined;
  const rootPicker = machine === null ? null : <Combobox label={t('directoryChoice.root')} query={rootSearch} onQueryChange={searchRoot}
    displayValue={root === '' ? undefined : selected ? root : t('directoryChoice.missingRoot')} loading={roots.isFetching} disabled={disabled || !roots.isSuccess || emptyRoots}
    error={rootError} fieldClassName="min-w-0 basis-32 grow" empty={t('directoryChoice.noRootsMatching')}
    aside={emptyRoots && machine.owner?.id === user ? <PathLink href={`/resources/machines/${encodeURIComponent(machine.id)}?tab=policy`} className="rounded-sm text-accent-text underline underline-offset-2">{t('environmentData.editPolicy')}</PathLink> : undefined}
    groups={[{ id: 'roots', label: '', options: offered.filter((path) => path.toLocaleLowerCase().includes(rootSearch.toLocaleLowerCase())).map((path) => ({ value: path, label: path, text: path })) }]}
    onOpenChange={(open) => { if (!open) searchRoot(''); }}
    onSelect={({ value: next }) => { setRoot(next); write(text, next); keepRoots(JSON.stringify([next, ...recentRoots.filter((old) => old !== next)])); searchRoot(''); }} />;
  return <DirectoryListing target={target} root={root} path={parent} prefix={prefix} recent={recent} enabled={source && opened && !disabled}>
    {(level) => <DirectoryFields target={target} root={root} value={text} current={trimEnd(decoded)} rootPicker={rootPicker} level={level}
      available={source} valid={rootReady} disabled={disabled || machine !== null && !rootReady} error={error}
      opened={opened && source} onOpenChange={(open) => setOpened(open && source)} onChange={write} onChoose={choose}
      browsing={browsing} onBrowsingChange={setBrowsing} recent={recentPaths} onReady={onReady} />}
  </DirectoryListing>;
}

interface FieldsProps {
  target: DirectoryTarget; root: string; value: string; current: string; rootPicker: ReactNode; level: DirectoryLevel;
  available: boolean; valid: boolean; disabled: boolean; error: unknown; opened: boolean; onOpenChange: (open: boolean) => void;
  onChange: (value: string) => void; onChoose: (value: string) => void; browsing: boolean; onBrowsingChange: (open: boolean) => void;
  recent: readonly string[]; onReady: Props['onReady'];
}
function DirectoryFields(props: FieldsProps) {
  const { t } = useTranslation();
  const { level, root, value } = props;
  const key = JSON.stringify([root, value]);
  const [kept, keep] = useState<{ key: string; problem: string | undefined }>({ key, problem: undefined });
  const observed = directoryProblem(level.issue);
  const retained = observed ?? (level.ready ? undefined : kept.key === key ? kept.problem : undefined);
  if (kept.key !== key || kept.problem !== retained) keep({ key, problem: retained });
  const problem = directoryProblem(props.error) ?? directoryProblem(level.issue) ?? (kept.key === key ? kept.problem : undefined);
  // A failed completion read does not remove manual entry; submission verifies the exact native path.
  const ready = props.valid;
  const onReady = props.onReady;
  useEffect(() => { onReady?.(ready); }, [onReady, ready]);
  const cut = value.lastIndexOf('/'), parent = cut < 0 ? '' : value.slice(0, cut);
  const choose = (name: string) => props.onChoose(parent === '' ? name : `${parent}/${name}`);
  return <div className="flex min-w-0 flex-1 flex-wrap items-end gap-2" data-directory-control>
    {props.rootPicker}
    <Combobox label={t(props.target.kind === 'machine' ? 'directoryChoice.subdirectory' : 'change.directory.label')}
      query={value} displayValue={value} onQueryChange={props.onChange} open={props.opened} onOpenChange={props.onOpenChange}
      groups={[{ id: 'directories', label: '', options: level.names.map((name) => ({ value: name, label: name, text: name, icon: <Folder aria-hidden /> })) }]}
      onSelect={({ value }) => choose(value)} onComplete={({ value }) => choose(value)}
      loading={level.pending} disabled={props.disabled} error={problem} placeholder={t('change.directory.root')}
      fieldClassName="min-w-0 basis-44 grow" empty={level.pending ? <SkeletonRows rows={2} /> : t('change.directory.empty')}
      footer={problem === undefined && level.pager === null ? undefined : <div className="flex items-center justify-between gap-2">{level.pager}{problem !== undefined && <Button size="sm" onClick={level.retry}>{t('common.tryAgain')}</Button>}</div>} />
    <Popover label={t('change.directory.browse')} open={props.browsing} onOpenChange={(open) => props.onBrowsingChange(open && props.available)}
      trigger={<Button size="sm" icon={<FolderOpen aria-hidden />} disabled={props.disabled} blocked={!props.available} reason={!props.available ? t('change.directory.relative') : undefined}>{t('change.directory.browse')}</Button>}>
      <DirectoryBrowser target={props.target} root={root} initial={props.current} recent={props.recent} onChoose={(path) => { props.onChoose(path); props.onBrowsingChange(false); }} />
    </Popover>
  </div>;
}

function DirectoryBrowser({ target, root, initial, recent, onChoose }: { target: DirectoryTarget; root: string; initial: string; recent: readonly string[]; onChoose: (path: string) => void }) {
  const { t } = useTranslation();
  const [path, setPath] = useState(initial);
  const names = childrenOf(recent, target, root, path);
  return <DirectoryListing target={target} root={root} path={path} prefix="" recent={names} enabled>
    {(level) => <div className="flex w-72 max-w-full flex-col gap-2">
      <div className="flex min-w-0 items-center gap-2"><IconButton size="sm" label={t('files.pages.openParent')} icon={<ArrowUp aria-hidden />} disabled={path === ''} onClick={() => setPath(path.slice(0, Math.max(0, path.lastIndexOf('/'))))} />
        <span className="min-w-0 flex-1 truncate text-sm" title={path || t('change.directory.root')}>{path || t('change.directory.root')}</span>{level.pager}</div>
      {level.pending ? <SkeletonRows rows={3} /> : directoryProblem(level.issue) !== undefined ? <ErrorState compact error={level.issue} onRetry={level.retry} /> : <ul className="max-h-64 overflow-y-auto" aria-label={t('change.directory.label')}>
        {level.names.map((name) => <li key={name}><Button variant="ghost" size="sm" icon={<Folder aria-hidden />} className="w-full justify-start" onClick={() => setPath(path === '' ? name : `${path}/${name}`)}>{name}</Button></li>)}
        {level.names.length === 0 && <li className="text-sm text-text-muted">{t('change.directory.empty')}</li>}
      </ul>}
      <Button size="sm" variant="primary" blocked={!level.ready} onClick={() => { if (level.ready) onChoose(path); }}>{t('change.directory.use')}</Button>
    </div>}
  </DirectoryListing>;
}
