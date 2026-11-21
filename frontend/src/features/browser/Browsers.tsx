/** The browsers of an environment and one of them in front: the list (open ones first, the thread's agent's before the rest, then the profiles none runs on), "Open a browser…", each way of being off and nothing found, and a profile renamed or deleted.
 *
 * The same view stands beside a thread and as the Browsers tab of the environment's page. The list
 * follows the environment's feed of browsers. What is on and what was found are two things shown
 * together: where browsers are on and nothing could run one, the view says so with what was looked for
 * and "Look again"; where they are off, by whom and where that is changed.
 */
import { useMutation, useQuery, useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { AppWindow, Ellipsis } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client as apiClient } from '@/api/client';
import { choiceCopy } from '@/i18n/choices';

import { BrowserProgramsMissing, browserKeys, browsersQuery, deleteProfile, lookAgain, renameProfile } from '@/api/browsers';
import type { Browser, BrowserProfile, Environment, EnvironmentBrowsers } from '@/generated/api-types';
import { socket } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';
import { SkeletonRows } from '@/ui/Skeleton';
import { Select } from '@/ui/Select';
import { useViewChoice } from '@/lib/viewChoices';
import { InteractiveFacts } from './InteractiveFacts';
import { StatusDot } from '@/ui/StatusDot';

import { Front } from './Front';
import { OpenDialog, PROFILE_NAME } from './OpenDialog';
import { useBrowserRequests } from './store';

/** The programs Orch looks for to start a browser, in the order it tries them. */
export const LOOKED_FOR = ['Playwright', 'chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable', 'microsoft-edge'] as const;

/** The list as the view shows it: open browsers, the thread's agent's first, then the others by their last use. */
export function orderBrowsers(browsers: readonly Browser[], threadId: string | null): Browser[] {
  const open = browsers.filter((browser) => browser.state !== 'closed');
  const mine = (browser: Browser) => threadId !== null && browser.thread?.id === threadId && browser.opened_by === null;
  return [...open.filter(mine), ...open.filter((browser) => !mine(browser))];
}

/** Keeps the list current from the environment's feed of browsers. */
function useBrowsersFeed(environmentId: string): void {
  const client = useQueryClient();
  useEffect(() => {
    const subscription = socket.subscribe(`env:${environmentId}:browsers`, {
      since: () => null,
      onFrame: (frame) => {
        if (frame.op !== 'event' && frame.op !== 'sync') return;
        void client.invalidateQueries({ queryKey: browserKeys.list(environmentId) });
      },
    });
    return () => subscription.unsubscribe();
  }, [client, environmentId]);
}

export function BrowsersPanel(props: { environment: Environment; threadId: string | null }) {
  const scope = useStore(apiClient.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  return <BrowserContents key={`${scope}:${props.environment.id}`} {...props} scope={scope} />;
}
function BrowserContents({ environment, threadId, scope }: { environment: Environment; threadId: string | null; scope: string }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [keptState, setKeptState] = useViewChoice<string>('browser-list', 'state', 'open', environment.id);
  const closed = keptState === 'closed';
  const [search, setSearch] = useState('');
  const canonical = useQuery({ ...browsersQuery(environment.id), enabled: false });
  const specification = browsersQuery(environment.id, { closed, query: search });
  const pageKey = [...specification.queryKey, 'pages', scope, closed, search];
  const query = useInfiniteQuery({ queryKey: pageKey, initialPageParam: null as string | null,
    queryFn: async (context): Promise<EnvironmentBrowsers> => {
      if (!closed && search === '' && context.pageParam === null) return client.query(browsersQuery(environment.id));
      const page = browsersQuery(environment.id, { closed, query: search, ...(context.pageParam === null ? {} : { cursor: context.pageParam }) });
      const read = page.queryFn;
      if (typeof read !== 'function') throw new Error('The browser reader is unavailable');
      return read({ ...context, queryKey: page.queryKey });
    }, getNextPageParam: (page) => page.next_cursor ?? undefined, placeholderData: (previous) => previous });
  useBrowsersFeed(environment.id);
  const [front, setFront] = useState<string | null>(null);
  const [opening, setOpening] = useState<{ url?: string; profile?: string } | null>(null);
  const [renaming, setRenaming] = useState<BrowserProfile | null>(null);
  const [deleting, setDeleting] = useState<BrowserProfile | null>(null);
  const requests = useBrowserRequests();
  // A deliberate Open from a tool can name a later page. Follow only that intent, never preload a list.
  const wanted = requests.front;
  const wantedPresent = query.data?.pages.some((page) => page.browsers.some((row) => row.id === wanted)) === true;
  const { isPending: reading, isFetchingNextPage: readingMore, isPlaceholderData: placeholder, hasNextPage: more, fetchNextPage: readNext } = query;
  useEffect(() => {
    if (wanted === null) return;
    if (closed) { setKeptState('open'); return; }
    if (!wantedPresent && !reading && !readingMore && !placeholder && more) void readNext();
  }, [wanted, closed, setKeptState, wantedPresent, reading, readingMore, placeholder, more, readNext]);
  const look = useMutation({ mutationFn: () => lookAgain(environment.id), onSuccess: (list) => { client.setQueryData(browserKeys.list(environment.id), list); client.setQueryData(pageKey, { pages: [list], pageParams: [null] }); } });
  const permissions = environment.access.permissions;
  const mayDrive = permissions.includes('browser.manage');

  if (query.isPending) return <SkeletonRows read={query} rows={4} />;
  if (query.isError) return <ErrorState compact error={query.error} message={query.error instanceof BrowserProgramsMissing ? t('browser.programsUnavailable') : undefined} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  const first = !closed && search === '' ? canonical.data ?? query.data.pages[0] : query.data.pages[0];
  if (first === undefined) return null;
  const held = [first, ...query.data.pages.slice(1)].flatMap((page) => page.browsers);
  const data = { ...first, browsers: [...new Map(held.map((row) => [row.id, row])).values()] };
  const matches = (text: string) => text.toLocaleLowerCase().includes(search.toLocaleLowerCase());
  const selected = data.browsers.filter((browser) => (browser.state === 'closed') === closed && matches(`${browser.profile ?? ''} ${browser.note ?? ''} ${browser.program ?? ''}`));
  const browsers = closed ? selected : orderBrowsers(selected, threadId);
  const idle = closed ? [] : data.profiles.filter((profile) => profile.browser === null && matches(profile.name));
  // What another screen asked (a browser in front, an address to open here) stands until the person chooses otherwise.
  const asked = requests.front !== null && browsers.some((browser) => browser.id === requests.front) ? requests.front : null;
  const askedOpen = requests.open !== null && requests.open.environmentId === environment.id && mayDrive ? requests.open : null;
  const shown = closed ? null : browsers.find((browser) => browser.id === (asked ?? front)) ?? browsers[0] ?? null;
  const dialog: { url?: string; profile?: string } | null = opening ?? (askedOpen === null ? null : { url: askedOpen.url });
  const choose = (id: string) => {
    requests.takeFront();
    setKeptState('open');
    setFront(id);
  };
  const endDialog = () => {
    requests.takeOpen();
    setOpening(null);
  };
  const found = data.support.programs.length > 0;
  const managers = environment.owner.display_name;
  const off = data.off;

  return (
    <div className="flex h-full min-h-0 flex-col gap-2 p-3" data-browsers-view={environment.id}>
      <div className="flex flex-wrap items-center gap-2">
        <h2 className="min-w-0 flex-1 truncate text-sm font-medium">{t('browser.list.title', { environment: environment.name })}</h2>
        {mayDrive && off === null && found && (
          <Button size="sm" icon={<AppWindow aria-hidden />} onClick={() => setOpening({})}>
            {t('browser.list.open')}
          </Button>
        )}
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Select size="sm" label={t('interactiveHistory.state')} value={closed ? 'closed' : 'open'} onValueChange={(value) => { requests.takeFront(); setKeptState(value); }}
          options={[{ value: 'open', label: t('browser.state.open') }, { value: 'closed', label: t('browser.state.closed') }]} />
        {(data.browsers.length + data.profiles.length > 10 || query.hasNextPage || search !== '') && <Input type="search" label={t('interactiveHistory.search')} value={search} onChange={(event) => { requests.takeFront(); setSearch(event.target.value); }} />}
      </div>
      {off !== null && <OffNotice off={off} environment={environment} />}
      {off === null && !found && (
        <InlineAlert
          tone="warning"
          title={t('browser.none.title', { environment: environment.name })}
          action={
            <Button action="refresh" size="sm" pending={look.isPending} onClick={() => look.mutate()}>
              {t('browser.none.lookAgain')}
            </Button>
          }
        >
          <span data-browsers-none>
            {t('browser.none.body')} {t('browser.none.lookedFor', { names: LOOKED_FOR.join(', ') })}
          </span>
        </InlineAlert>
      )}
      {look.error !== null && <ErrorAlert error={look.error} message={look.error instanceof BrowserProgramsMissing ? t('browser.programsUnavailable') : undefined} />}
      {data.support.sandbox === false && <p className="text-xs text-text-muted">{t('browser.noSandbox')}</p>}
      {(browsers.length > 0 || idle.length > 0) && (
        // Keep the profile selector short enough that adding a row cannot consume the picture's room.
        <ul className={cx('flex shrink-0 flex-col overflow-auto rounded-lg border border-border', shown === null ? 'max-h-48' : 'max-h-[min(12rem,25%)]')} aria-label={t('browser.list.label')}>
          {browsers.map((browser) => (
            <li key={browser.id} className={cx('flex items-start gap-2 border-b border-border px-2 py-1 last:border-0', browser.id === shown?.id && 'bg-bg-subtle')} data-browser-row={browser.id}>
              <StatusDot className="h-(--control-sm)" tone={browser.state === 'open' ? 'success' : 'info'} label={t(`browser.state.${browser.state}`)} />
              <button type="button" className="min-w-0 flex-1 text-left" aria-pressed={browser.id === shown?.id} disabled={browser.state === 'closed'} onClick={() => choose(browser.id)}>
                <span className="block truncate text-sm font-medium leading-(--control-sm)">{browser.profile ?? t('browser.list.ownProfile')}</span>
                <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted">
                  <span>{browser.opened_by !== null ? t('browser.list.openedBy', { name: browser.opened_by.display_name }) : browser.thread !== null ? t('browser.list.openedByAgent', { thread: browser.thread.name ?? t('browser.list.hiddenThread') }) : t('browser.list.announced')}</span>
                  {browser.preset !== null && <span>{choiceCopy(t, `browserMode.${browser.preset}`).name}</span>}
                  {browser.tabs !== null && <span>{t('browser.list.tabs', { count: browser.tabs })}</span>}
                  {browser.note !== null && <span className="wrap-anywhere">{browser.note}</span>}
                </span>
              </button>
              {browser.thread?.name && (
                <Link to="/thread/$threadId" params={{ threadId: browser.thread.id }} className="hidden shrink-0 text-xs text-accent-text underline sm:inline">
                  {browser.thread.name}
                </Link>
              )}
              <InteractiveFacts subject={browser} />
              {!closed && browser.profile !== null && <ProfileMenu profile={data.profiles.find((one) => one.name === browser.profile) ?? null} running mayManage={mayDrive} onRename={setRenaming} onDelete={setDeleting} />}
            </li>
          ))}
          {idle.map((profile) => (
            <li key={profile.name} className="flex items-start gap-2 border-b border-border px-2 py-1 last:border-0" data-profile-row={profile.name}>
              <StatusDot className="h-(--control-sm)" tone="idle" label={t('browser.list.notOpen')} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm leading-(--control-sm)">{profile.name}</span>
                <span className="block text-xs text-text-muted">{t('browser.list.profileNotOpen')}</span>
              </span>
              {mayDrive && off === null && found && (
                <Button size="sm" variant="ghost" onClick={() => setOpening({ profile: profile.name })}>
                  {t('browser.list.openOne')}
                </Button>
              )}
              <ProfileMenu profile={profile} running={false} mayManage={mayDrive} onRename={setRenaming} onDelete={setDeleting} />
            </li>
          ))}
        </ul>
      )}
      {query.hasNextPage && <Button size="sm" pending={query.isFetchingNextPage} disabled={query.isPlaceholderData} onClick={() => void query.fetchNextPage()}>{t('interactiveHistory.more')}</Button>}
      {closed && !query.isPlaceholderData && browsers.length === 0 && <EmptyState compact title={t('interactiveHistory.empty')} />}
      {!closed && browsers.length === 0 && idle.length === 0 && off === null && found && <EmptyState compact icon={<AppWindow aria-hidden />} title={t('browser.list.empty')} description={mayDrive ? t('browser.list.emptyHint') : undefined} />}
      {shown !== null ? (
        <Front key={shown.id} browser={shown} list={data} mayDrive={mayDrive} environmentName={environment.name} managers={managers} mayReadFiles={permissions.includes('files.view')} />
      ) : null}
      {dialog !== null && (
        <OpenDialog
          environmentId={environment.id}
          environmentName={environment.name}
          list={data}
          watchers={environment.reach}
          threadId={threadId}
          {...(dialog.url === undefined ? {} : { url: dialog.url })}
          {...(dialog.profile === undefined ? {} : { profile: dialog.profile })}
          onClose={endDialog}
          onOpened={(browser) => {
            endDialog();
            choose(browser.id);
          }}
        />
      )}
      {renaming !== null && <RenameProfile environmentId={environment.id} profile={renaming} onClose={() => setRenaming(null)} />}
      {deleting !== null && <DeleteProfile environmentId={environment.id} profile={deleting} onClose={() => setDeleting(null)} />}
    </div>
  );
}

/** Why browsers are off here, by whom, and where that is changed. */
function OffNotice({ off, environment }: { off: NonNullable<EnvironmentBrowsers['off']>; environment: Environment }) {
  const { t } = useTranslation();
  const words = {
    disabled_by_policy: t('browser.off.policy', { environment: environment.name }),
    environment_switch_off: t('browser.off.switch', { environment: environment.name, owner: environment.owner.display_name }),
    machine_policy: t('browser.off.machine', { machine: environment.machine?.name ?? environment.name }),
    environment_offline: t('browser.off.offline', { environment: environment.name }),
  }[off];
  return (
    <InlineAlert tone={off === 'environment_offline' ? 'info' : 'warning'}>
      <span data-browsers-off={off}>{words}</span>
    </InlineAlert>
  );
}

function ProfileMenu({ profile, running, mayManage, onRename, onDelete }: { profile: BrowserProfile | null; running: boolean; mayManage: boolean; onRename: (profile: BrowserProfile) => void; onDelete: (profile: BrowserProfile) => void }) {
  const { t } = useTranslation();
  if (profile === null || !mayManage) return null;
  const busy = running || profile.browser !== null ? t('browser.profile.inUse') : false;
  return (
    <Menu
      label={t('browser.profile.menu', { name: profile.name })}
      align="end"
      entries={[
        { id: 'rename', action: 'rename', label: t('browser.profile.rename'), disabled: busy, onSelect: () => onRename(profile) },
        { id: 'delete', action: 'delete', label: t('browser.profile.delete'), danger: true, disabled: busy, onSelect: () => onDelete(profile) },
      ]}
      trigger={<IconButton size="sm" label={t('browser.profile.menu', { name: profile.name })} icon={<Ellipsis aria-hidden />} />}
    />
  );
}

function RenameProfile({ environmentId, profile, onClose }: { environmentId: string; profile: BrowserProfile; onClose: () => void }) {
  const { t } = useTranslation();
  const [name, setName] = useState(profile.name);
  const rename = useMutation({
    mutationFn: () => renameProfile(environmentId, profile.name, name.trim()),
    onSuccess: () => {
      onClose();
    },
  });
  const fault = PROFILE_NAME.test(name.trim()) ? null : t('browser.open.nameFault');
  return (
    <Dialog
      open
      size="sm"
      title={t('browser.profile.renameTitle', { name: profile.name })}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button action="rename" variant="primary" pending={rename.isPending} disabled={fault !== null || name.trim() === profile.name} onClick={() => rename.mutate()}>
            {t('browser.profile.renameDo')}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (fault === null) rename.mutate(); }}>
        <Input label={t('browser.open.name')} hint={t('browser.open.nameHint')} mono value={name} error={fault ?? undefined} onChange={(event) => setName(event.target.value.toLowerCase())} />
        {rename.error !== null && <ErrorAlert error={rename.error} />}
      </form>
    </Dialog>
  );
}

function DeleteProfile({ environmentId, profile, onClose }: { environmentId: string; profile: BrowserProfile; onClose: () => void }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState('');
  const remove = useMutation({
    mutationFn: () => deleteProfile(environmentId, profile.name),
    onSuccess: () => {
      onClose();
    },
  });
  const named = typed === profile.name;
  return (
    <Dialog
      open
      size="sm"
      title={t('browser.profile.deleteTitle', { name: profile.name })}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button action="delete" variant="danger" pending={remove.isPending} disabled={!named} onClick={() => remove.mutate()}>
            {t('browser.profile.deleteDo')}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (named) remove.mutate(); }}>
        <p className="text-sm">{t('browser.profile.deleteBody')}</p>
        <Input label={t('browser.profile.typeName', { name: profile.name })} mono value={typed} autoComplete="off" onChange={(event) => setTyped(event.target.value)} />
        {remove.error !== null && <ErrorAlert error={remove.error} />}
      </form>
    </Dialog>
  );
}

/** Whether the view's tab needs its warning mark: browsers are on and nothing was found there that could run one. */
export function browsersMissing(list: EnvironmentBrowsers | undefined): boolean {
  return list?.off === null && list.support.programs.length === 0;
}
