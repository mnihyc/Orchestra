/** The page of `/dev/ui`: each shared control that is built, drawn in the light and the dark theme side by side, with the density and the language of the whole page to choose. */
import { useRouter } from '@tanstack/react-router';
import {
  Archive,
  Diamond,
  ExternalLink,
  Folder,
  GitBranch,
  LoaderCircle,
  MessageSquareText,
  Monitor,
  Moon,
  Pencil,
  Plus,
  RefreshCw,
  ScrollText,
  RotateCcw,
  Share2,
  ShieldCheck,
  Sun,
  Users,
} from 'lucide-react';
import { type ReactNode, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { installDevCatalogue } from '@/i18n/dev';

installDevCatalogue();

import { ApiError } from '@/api/client';
import type { Density, EnvironmentStatus, Field, HistoryLine, Locale, MissingAccess, PresenceUser, Problem, Theme, UserRef } from '@/generated/api-types';
import { LOCALES } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { languageOf } from '@/features/files/languages';
import { denial } from '@/lib/permissions';
import { NetworkError } from '@/lib/requests';
import { usePageTitle } from '@/lib/title';
import { Avatar } from '@/ui/Avatar';
import { AvatarStack } from '@/ui/AvatarStack';
import { Badge } from '@/ui/Badge';
import { Banner } from '@/ui/Banner';
import { Breadcrumbs } from '@/ui/Breadcrumbs';
import { Button } from '@/ui/Button';
import { actionName } from '@/ui/actions';
import { ByteSize } from '@/ui/ByteSize';
import { Checkbox } from '@/ui/Checkbox';
import { Combobox, type ComboboxGroup } from '@/ui/Combobox';
import { CodeView } from '@/ui/CodeView';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { ContextMenu } from '@/ui/ContextMenu';
import { CopyButton } from '@/ui/CopyButton';
import { Dialog } from '@/ui/Dialog';
import { Drawer } from '@/ui/Drawer';
import { EmojiPicker } from '@/ui/EmojiPicker';
import { EmptyState } from '@/ui/EmptyState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { FieldForm, type FormValues, initialValues } from '@/ui/FieldForm';
import { HelpHint } from '@/ui/HelpHint';
import { HistoryLines } from '@/ui/HistoryLines';
import { HolderChip, type PeopleSource } from '@/ui/HolderChip';
import { IconButton } from '@/ui/IconButton';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input, PasswordInput } from '@/ui/Input';
import { Kbd } from '@/ui/Kbd';
import { LockBadge } from '@/ui/LockBadge';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Meter } from '@/ui/Meter';
import { NODE_COLOR_NAMES, NodeIcon } from '@/ui/NodeIcon';
import { Pagination } from '@/ui/Pagination';
import { PermissionGate } from '@/ui/PermissionGate';
import { Popover } from '@/ui/Popover';
import { PresenceStack } from '@/ui/PresenceStack';
import { ProvenancePill } from '@/ui/ProvenancePill';
import { QrCode } from '@/ui/QrCode';
import { RadioGroup } from '@/ui/RadioGroup';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Select } from '@/ui/Select';
import { Skeleton, SkeletonRows } from '@/ui/Skeleton';
import { Sparkline } from '@/ui/Sparkline';
import { StateChip, StateIcon, type WorkState } from '@/ui/StateChip';
import { StatusBar, StatusPart } from '@/ui/StatusBar';
import { StatusDot } from '@/ui/StatusDot';
import { Switch } from '@/ui/Switch';
import { Table } from '@/ui/Table';
import { Tabs } from '@/ui/Tabs';
import { TagChip } from '@/ui/TagChip';
import { Textarea } from '@/ui/Textarea';
import { TimeSeriesChart } from '@/ui/TimeSeriesChart';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';
import { Tree, type TreeRow } from '@/ui/Tree';
import { PersonCard } from '@/ui/PersonCard';
import { UserChip } from '@/ui/UserChip';

/** Names that are the same in every language: the things and the people the samples are about. */
const NAMES = {
  org: 'Acme',
  department: 'Research',
  team: 'ML',
  project: 'Web',
  branch: 'fix/login ↑2',
  requestId: '01JB2M7Q4X8Z',
  ada: 'Ada',
  grace: 'Grace',
  ming: '王小明',
  sally: 'Sally',
  bot: 'Deploy bot',
  agent: 'Aster',
  workspace: 'Workspace',
  tagBug: 'bug',
  tagUi: 'ui',
};

const THEMES = ['light', 'dark'] as const;

const STATES: readonly WorkState[] = ['running', 'starting', 'waiting_approval', 'blocked', 'failed', 'idle', 'done', 'archived', 'offline'];

const ENVIRONMENT_STATES: readonly EnvironmentStatus[] = ['running', 'provisioning', 'recreating', 'moving', 'stopped', 'none', 'offline', 'failed', 'disabled'];

const refused = new ApiError(
  {
    type: 'orch:error:provider.unavailable',
    code: 'provider.unavailable',
    title: 'Not ready',
    status: 503,
    detail: 'the provider is not ready',
    request_id: NAMES.requestId,
  } satisfies Problem,
  null,
);
const lost = new NetworkError('offline');

/** The people the samples of places are about; their names are the same in every language. */
const person = (n: number, kind: UserRef['kind'], username: string, name: string, text: string): UserRef => ({
  id: `00000000-0000-4000-8000-${n.toString().padStart(12, '0')}`,
  kind,
  username,
  display_name: name,
  avatar: { text, color: n * 2 },
});
const ADA = person(1, 'human', 'ada', NAMES.ada, 'A');
const GRACE = person(2, 'human', 'grace', NAMES.grace, 'G');
const MING = person(3, 'human', 'ming', NAMES.ming, '王');
const SALLY = person(4, 'human', 'sally', NAMES.sally, 'S');
const PEOPLE: readonly UserRef[] = [ADA, GRACE, MING, SALLY];
const BOT = person(5, 'service', 'deploy', NAMES.bot, 'D');

/** The chart specimen's day: a value an hour for the hours of one day, the same in every language. */
const CHART_FROM = Date.UTC(2026, 0, 1);
const CHART_TO = CHART_FROM + 24 * 3_600_000;
const hourly = (value: (hour: number) => number): [number, number][] =>
  Array.from({ length: 25 }, (_, hour) => [CHART_FROM + hour * 3_600_000, value(hour)]);
const CHART_COMPLETED = hourly((hour) => Math.round(6 + 4 * Math.sin(hour / 3.5)));
const CHART_FAILED = hourly((hour) => (hour % 7 === 3 ? 2 : hour % 5 === 0 ? 1 : 0));
/** People who are looking at something, since a fixed moment. */
const looking = (users: readonly UserRef[]): PresenceUser[] => users.map((user) => ({ user, since: '2026-10-01T09:00:00Z' }));

/** What the server lists as missing when it refuses an act on a project: the permission, on what, and whom to ask. */
const MISSING: MissingAccess = {
  object: { id: '00000000-0000-4000-8003-000000000002', kind: 'project', name: NAMES.project, owner: ADA },
  permissions: ['project.interact'],
  ask: [ADA, GRACE],
  ask_more: 2,
  override: false,
  closed: null,
  options: null,
};
const denied = new ApiError(
  {
    type: 'orch:error:authz.denied',
    code: 'authz.denied',
    title: 'Denied',
    status: 403,
    detail: 'the caller lacks a permission',
    request_id: NAMES.requestId,
    required: 'project.interact',
    requestable: true,
    override: false,
    missing: [MISSING],
  } satisfies Problem,
  null,
);

/** One control under its name, drawn twice: in the light theme and in the dark one. */
function Specimen({ name, children, wide = false }: { name: string; children: ReactNode; wide?: boolean }) {
  return (
    <section aria-label={name} className="flex flex-col gap-2">
      <h2 className="font-mono text-sm font-medium text-text-muted">{name}</h2>
      <div className={wide ? 'flex flex-col gap-3' : 'grid grid-cols-1 gap-3 lg:grid-cols-2'}>
        {THEMES.map((theme) => (
          <div
            key={theme}
            data-theme={theme}
            className="flex flex-col gap-4 rounded-lg border border-border bg-bg p-4 text-base text-text"
          >
            {children}
          </div>
        ))}
      </div>
    </section>
  );
}

const Row = ({ children }: { children: ReactNode }) => <div className="flex flex-wrap items-center gap-3">{children}</div>;

interface SampleNode {
  name: string;
  kind: 'org' | 'project' | 'topic' | 'thread';
}

type FullMode = 'warn' | 'block' | 'system';

/** A hint beside a term, a field and a switch: the card with its four parts, and the "?" in the line of a label. */
function HintSample() {
  const { t } = useTranslation();
  const [on, setOn] = useState(false);
  const hint = (term: string) => <HelpHint term={term} what={t('resources.gallery.hintWhat')} which={t('resources.gallery.hintWhich')} />;
  return (
    <>
      <Row>
        <span className="text-sm font-medium text-text">{t('resources.gallery.hintTerm')}</span>
        <HelpHint
          term={t('resources.gallery.hintTerm')}
          what={t('resources.gallery.hintWhat')}
          which={t('resources.gallery.hintWhich')}
          where={t('resources.gallery.hintWhere')}
          more={{ label: t('help.hint.more'), onOpen: () => undefined }}
        />
      </Row>
      <Input label={t('resources.gallery.hintField')} help={hint(t('resources.gallery.hintField'))} defaultValue="Workspace" />
      <Switch label={t('resources.gallery.hintSwitch')} help={hint(t('resources.gallery.hintSwitch'))} checked={on} onCheckedChange={setOn} />
    </>
  );
}

/** A choice among options that explain themselves, as a list and as cards, with one that cannot be chosen. */
function ChoiceSample() {
  const { t } = useTranslation();
  const [value, setValue] = useState<FullMode | null>('warn');
  const options = [
    { value: 'warn' as const, label: t('resources.gallery.warn'), description: t('resources.gallery.warnLead') },
    { value: 'block' as const, label: t('resources.gallery.block'), description: t('resources.gallery.blockLead') },
    { value: 'system' as const, label: t('resources.gallery.system'), disabled: true, note: t('resources.gallery.systemWhy') },
  ];
  return (
    <>
      <p className="text-sm text-text-muted">{t('resources.gallery.asList')}</p>
      <RadioGroup label={t('resources.gallery.choice')} variant="list" value={value} onValueChange={setValue} options={options} />
      <p className="text-sm text-text-muted">{t('resources.gallery.asCards')}</p>
      <RadioGroup label={t('resources.gallery.choice')} value={value} onValueChange={setValue} options={options} />
    </>
  );
}

/** A form drawn from what a driver declares: each kind of field, one of them in error. */
function DeclaredFieldsSample() {
  const { t } = useTranslation();
  const fields: Field[] = [
    { name: 'dockerfile', label: t('resources.gallery.fieldDockerfile'), type: 'secret_text', required: true, help: t('resources.gallery.fieldDockerfileHelp'), max_bytes: 4096, choices: null, default: null },
    { name: 'tag', label: t('resources.gallery.fieldTag'), type: 'text', required: false, help: null, max_bytes: null, choices: null, default: 'latest' },
    { name: 'token', label: t('resources.gallery.fieldToken'), type: 'secret', required: true, help: null, max_bytes: null, choices: null, default: null },
    { name: 'pull', label: t('resources.gallery.fieldPull'), type: 'boolean', required: false, help: t('resources.gallery.fieldPullHelp'), max_bytes: null, choices: null, default: false },
    { name: 'runtime', label: t('resources.gallery.fieldRuntime'), type: 'choice', required: false, help: null, max_bytes: null, choices: [{ value: 'runc', label: 'runc' }, { value: 'runsc', label: 'gVisor' }], default: null },
    { name: 'args', label: t('resources.gallery.fieldArgs'), type: 'map', required: false, help: null, max_bytes: null, choices: null, default: null },
    { name: 'headers', label: t('resources.gallery.fieldHeaders'), type: 'secret_map', required: false, help: null, max_bytes: null, choices: null, default: null },
  ];
  const [values, setValues] = useState<FormValues>(() => ({ ...initialValues(fields), args: { NODE_ENV: 'production' } }));
  return <FieldForm fields={fields} values={values} errors={{ token: t('resources.gallery.fieldTokenError') }} onChange={(name, value) => setValues((before) => ({ ...before, [name]: value }))} />;
}

export default function Gallery() {
  const { t } = useTranslation();
  const router = useRouter();
  usePageTitle(t('dev.title'));
  const theme = useAppearance((state) => state.theme);
  const density = useAppearance((state) => state.density);
  const locale = useAppearance((state) => state.locale);
  const set = useAppearance((state) => state.set);

  const [name, setName] = useState('');
  const [tell, setTell] = useState(true);
  const [awake, setAwake] = useState(false);
  const [view, setView] = useState<'list' | 'board'>('list');
  const [format, setFormat] = useState<'markdown' | 'html'>('markdown');
  const [emoji, setEmoji] = useState('');
  const [level, setLevel] = useState('important');
  const [focusMode, setFocusMode] = useState(false);
  const [dialog, setDialog] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [drawer, setDrawer] = useState(false);
  const [tab, setTab] = useState<'overview' | 'members' | 'activity' | 'usage'>('members');
  const [role, setRole] = useState<'viewer' | 'developer' | 'admin'>('viewer');
  const [description, setDescription] = useState(t('dev.sample.descriptionValue'));
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set(['research', 'web']));
  const chartLines = useMemo(
    () => [
      { id: 'completed', label: t('admin.overview.results.completed'), slot: 1, points: CHART_COMPLETED },
      { id: 'failed', label: t('admin.overview.results.failed'), slot: 2, points: CHART_FAILED },
    ],
    [t],
  );
  // The rows of a tree of ticks that are ticked by themselves; the row above them is half ticked while some are.
  const [ticked, setTicked] = useState<ReadonlySet<string>>(new Set(['web']));

  const entries: MenuEntry[] = [
    { id: 'open', label: t('dev.sample.open'), onSelect: () => undefined },
    { id: 'tab', label: t('dev.sample.openNewTab'), icon: <ExternalLink aria-hidden />, shortcut: 'Mod+Enter', onSelect: () => undefined },
    { id: 'rename', label: t('dev.sample.rename'), icon: <Pencil aria-hidden />, shortcut: 'F2', onSelect: () => setDialog(true) },
    { id: 'share', label: t('dev.sample.share'), icon: <Share2 aria-hidden />, disabled: t('dev.sample.shareOff'), onSelect: () => undefined },
    { type: 'check', id: 'focus-mode', label: t('workbench.workspace.focus'), checked: focusMode, onCheckedChange: setFocusMode },
    {
      type: 'sub',
      id: 'level',
      label: t('dev.sample.level'),
      entries: [
        {
          type: 'radio',
          id: 'levels',
          label: t('dev.sample.level'),
          value: level,
          onValueChange: setLevel,
          options: [
            { value: 'all', label: t('dev.sample.levelAll') },
            { value: 'important', label: t('dev.sample.levelImportant') },
            { value: 'off', label: t('dev.sample.levelOff') },
          ],
        },
      ],
    },
    { type: 'separator', id: 'separator' },
    { id: 'archive', label: t('dev.sample.archiveEntry'), icon: <Archive aria-hidden />, danger: true, onSelect: () => setConfirm(true) },
  ];

  const nodes: (TreeRow<SampleNode> & { under?: string })[] = [
    { id: 'research', depth: 0, parentId: null, expandable: true, expanded: expanded.has('research'), item: { name: NAMES.department, kind: 'org' } },
    { id: 'web', depth: 1, parentId: 'research', expandable: true, expanded: expanded.has('web'), item: { name: NAMES.project, kind: 'project' }, under: 'research' },
    { id: 'fix', depth: 2, parentId: 'web', expandable: true, expanded: expanded.has('fix'), item: { name: t('dev.sample.nameValue'), kind: 'topic' }, under: 'web' },
    { id: 'main', depth: 3, parentId: 'fix', expandable: false, expanded: false, item: { name: t('dev.sample.thread'), kind: 'thread' }, under: 'fix' },
  ];
  // A tree of ticks: a project with two topics to choose and a thread that may be read and not chosen.
  const chosen = ['fix', 'main'].filter((id) => ticked.has(id));
  const tickRows: TreeRow<SampleNode>[] = [
    { id: 'web', depth: 0, parentId: null, expandable: true, expanded: true, tick: chosen.length === 2 ? true : chosen.length > 0 ? 'mixed' : false, item: { name: NAMES.project, kind: 'project' } },
    { id: 'fix', depth: 1, parentId: 'web', expandable: false, expanded: false, tick: ticked.has('fix'), item: { name: t('dev.sample.nameValue'), kind: 'topic' } },
    { id: 'main', depth: 1, parentId: 'web', expandable: false, expanded: false, tick: ticked.has('main'), item: { name: t('dev.sample.thread'), kind: 'thread' } },
    { id: 'locked', depth: 1, parentId: 'web', expandable: false, expanded: false, tick: false, tickLocked: true, item: { name: NAMES.department, kind: 'org' } },
  ];
  // A row is in view when every row above it is open.
  const rows = nodes.filter((row) => {
    for (let above = row.under; above !== undefined; above = nodes.find((node) => node.id === above)?.under) {
      if (!expanded.has(above)) return false;
    }
    return true;
  });

  return (
    <div className="min-h-dvh bg-bg-inset text-text">
      <header className="sticky top-0 z-(--layer-sticky) flex flex-wrap items-center gap-x-6 gap-y-3 border-b border-border bg-surface px-6 py-3">
        <div className="mr-auto flex min-w-0 flex-col">
          <h1 className="text-xl font-semibold">{t('dev.title')}</h1>
          <p className="text-sm text-text-muted">{t('dev.lead')}</p>
        </div>
        <SegmentedControl<Theme>
          size="sm"
          label={t('preferences.theme')}
          value={theme}
          onValueChange={(value) => set({ theme: value })}
          segments={[
            { value: 'system', label: t('preferences.themeSystem'), icon: <Monitor aria-hidden /> },
            { value: 'light', label: t('preferences.themeLight'), icon: <Sun aria-hidden /> },
            { value: 'dark', label: t('preferences.themeDark'), icon: <Moon aria-hidden /> },
          ]}
        />
        <SegmentedControl<Density>
          size="sm"
          label={t('preferences.density')}
          value={density}
          onValueChange={(value) => set({ density: value })}
          segments={[
            { value: 'comfortable', label: t('preferences.densityComfortable') },
            { value: 'compact', label: t('preferences.densityCompact') },
          ]}
        />
        <SegmentedControl<Locale>
          size="sm"
          label={t('preferences.language')}
          value={locale}
          onValueChange={(value) => set({ locale: value })}
          segments={LOCALES.map((entry) => ({ value: entry.locale, label: entry.name }))}
        />
      </header>

      <main className="mx-auto flex max-w-(--breakpoint-xl) flex-col gap-8 px-6 py-8">
        <Specimen name="Button">
          <Row>
            <Button variant="primary">{t('dev.sample.save')}</Button>
            <Button>{t('common.cancel')}</Button>
            <Button variant="ghost" icon={<RefreshCw aria-hidden />}>
              {t('dev.sample.refresh')}
            </Button>
            <Button variant="danger" icon={<Archive aria-hidden />}>
              {t('dev.sample.archive')}
            </Button>
          </Row>
          <Row>
            <Button variant="primary" size="sm" icon={<Plus aria-hidden />}>
              {t('dev.sample.newTopic')}
            </Button>
            <Button size="sm">{t('common.cancel')}</Button>
            <Button variant="ghost" size="sm">
              {t('common.details')}
            </Button>
            <Button variant="danger" size="sm">
              {t('dev.sample.archiveEntry')}
            </Button>
          </Row>
          <Row>
            <Button variant="primary" pending>
              {t('dev.sample.save')}
            </Button>
            <Button pending>{t('dev.sample.refresh')}</Button>
            <Button variant="primary" disabled>
              {t('dev.sample.save')}
            </Button>
            <Button disabled>{t('common.cancel')}</Button>
          </Row>
        </Specimen>

        <Specimen name="IconButton, Tooltip, Kbd, CopyButton">
          <Row>
            <IconButton label={t('dev.sample.refresh')} icon={<RefreshCw aria-hidden />} shortcut="Mod+R" />
            <IconButton label={t('dev.sample.newTopic')} icon={<Plus aria-hidden />} variant="secondary" />
            <IconButton label={t('preferences.themeLight')} icon={<Sun aria-hidden />} pressed size="sm" />
            <IconButton label={t('dev.sample.refresh')} icon={<RefreshCw aria-hidden />} pending size="sm" />
            <IconButton label={t('dev.sample.archiveEntry')} icon={<Archive aria-hidden />} disabled />
            <Tooltip content={t('dev.sample.nameHint')} shortcut="F2">
              <button type="button" className="rounded-sm text-sm text-accent-text underline underline-offset-2">
                {t('dev.sample.tooltip')}
              </button>
            </Tooltip>
          </Row>
          <Row>
            <Kbd shortcut="Mod+K" />
            <Kbd shortcut="Shift+F6" />
            <Kbd shortcut="G I" />
            <Kbd shortcut="Alt+ArrowUp" />
            <span className="inline-flex items-center gap-1 font-mono text-sm">
              {NAMES.requestId}
              <CopyButton value={NAMES.requestId} label={t('dev.sample.copy')} />
            </span>
          </Row>
        </Specimen>

        <Specimen name="Input, PasswordInput">
          <Input
            label={t('dev.sample.name')}
            hint={t('dev.sample.nameHint')}
            value={name}
            placeholder={t('dev.sample.nameValue')}
            onChange={(event) => setName(event.target.value)}
          />
          <Input label={t('dev.sample.name')} error={t('dev.sample.nameError')} defaultValue={t('dev.sample.nameValue')} />
          <Input label={t('auth.fields.email')} optional type="email" hint={t('auth.register.emailHint')} />
          <PasswordInput label={t('auth.fields.password')} hint={t('auth.fields.passwordHint', { min: 10 })} defaultValue="correct horse" />
          <Input label={t('dev.sample.name')} disabled defaultValue={t('dev.sample.nameValue')} hint={t('dev.sample.locked')} />
        </Specimen>

        <Specimen name="Checkbox, Switch, SegmentedControl">
          <Checkbox label={t('dev.sample.tell')} hint={t('dev.sample.tellHint')} checked={tell} onCheckedChange={setTell} />
          <Checkbox label={t('dev.sample.tell')} checked="indeterminate" onCheckedChange={() => undefined} />
          <Checkbox label={t('dev.sample.tell')} checked disabled onCheckedChange={() => undefined} />
          <Switch label={t('dev.sample.keepAwake')} hint={t('dev.sample.keepAwakeHint')} checked={awake} onCheckedChange={setAwake} />
          <Switch label={t('dev.sample.keepAwake')} checked disabled onCheckedChange={() => undefined} />
          <Row>
            <SegmentedControl
              label={t('dev.sample.view')}
              value={view}
              onValueChange={setView}
              segments={[
                { value: 'list', label: t('dev.sample.list') },
                { value: 'board', label: t('dev.sample.board') },
              ]}
            />
            <SegmentedControl
              size="sm"
              label={t('dev.sample.view')}
              value={view}
              onValueChange={setView}
              segments={[
                { value: 'list', label: t('dev.sample.list') },
                { value: 'board', label: t('dev.sample.board'), disabled: true },
              ]}
            />
          </Row>
        </Specimen>

        <Specimen name="InlineAlert, Banner">
          <InlineAlert tone="info">{t('dev.sample.info')}</InlineAlert>
          <InlineAlert tone="success">{t('dev.sample.success')}</InlineAlert>
          <InlineAlert tone="warning">{t('dev.sample.warning')}</InlineAlert>
          <InlineAlert
            tone="danger"
            title={t('dev.sample.dangerTitle')}
            action={<Button size="sm">{t('common.tryAgain')}</Button>}
          >
            {t('dev.sample.danger')}
          </InlineAlert>
          <div className="overflow-hidden rounded-md border border-border">
            <Banner tone="info" onDismiss={() => undefined}>
              {t('dev.sample.banner')}
            </Banner>
            <Banner tone="warning">{t('shell.banner.reconnecting')}</Banner>
            <Banner tone="danger" action={<Button size="sm">{t('shell.banner.reload')}</Button>}>
              {t('shell.banner.newVersion')}
            </Banner>
          </div>
        </Specimen>

        <Specimen name="StateChip, StateIcon, StatusDot, Badge">
          <Row>
            {STATES.map((state) => (
              <StateChip key={state} state={state} />
            ))}
          </Row>
          <Row>
            {STATES.map((state) => (
              <StateIcon key={state} state={state} />
            ))}
            <StatusDot tone="success" label={t('dev.sample.online')} showLabel />
            <StatusDot tone="warning" label={t('dev.sample.slow')} showLabel />
            <StatusDot tone="off" label={t('state.offline')} showLabel />
            <StatusDot tone="danger" label={t('state.failed')} />
          </Row>
          <Row>
            <Badge tone="accent" count={3} label={t('dev.sample.unread')} />
            <Badge tone="danger" count={128} />
            <Badge tone="warning" count={1} label={t('dev.sample.waiting')} />
            <Badge tone="neutral" count={12} />
            <Badge tone="neutral">{t('dev.sample.beta')}</Badge>
          </Row>
        </Specimen>

        <Specimen name="NodeIcon">
          <Row>
            {PLACE_KINDS.map((kind) => (
              <NodeIcon key={kind} kind={kind} label={t(`navigation.row.${kind}`)} />
            ))}
            <NodeIcon kind="topic" done label={t('navigation.row.topicDone')} />
            <NodeIcon kind="thread" label={t('navigation.row.thread')} />
            <NodeIcon kind="document" label={t('navigation.row.document')} />
          </Row>
          <Row>
            {NODE_COLOR_NAMES.map((name, index) => (
              <NodeIcon key={name} kind="folder" color={index + 1} label={t(`places.colour.names.${name}`)} />
            ))}
          </Row>
        </Specimen>

        <Specimen name="EnvironmentDot">
          <div className="flex flex-col gap-2">
            {ENVIRONMENT_STATES.map((status) => (
              <EnvironmentDot key={status} status={status} showLabel />
            ))}
          </div>
          <Row>
            {ENVIRONMENT_STATES.map((status) => (
              <EnvironmentDot key={status} status={status} name={NAMES.workspace} />
            ))}
          </Row>
        </Specimen>

        <Specimen name="Avatar, RelativeTime">
          <Row>
            <Avatar text="A" color={0} size={24} label={NAMES.ada} person={ADA} />
            <Avatar text="G" color={3} size={28} label={NAMES.grace} />
            <Avatar text="王" color={7} size={32} label={NAMES.ming} />
            <Avatar text="👩‍🚀" color={9} size={40} label={NAMES.sally} />
            <Avatar text="D" color={5} size={32} kind="service" label={NAMES.bot} />
            <Avatar text="A" color={8} size={32} kind="org" label={NAMES.org} />
            <Avatar text="" color={0} size={32} kind="agent" mark={<MessageSquareText aria-hidden />} label={NAMES.agent} />
            <Avatar text="R" color={11} size="row" label={NAMES.department} />
          </Row>
          <Row>
            <RelativeTime at={new Date(Date.now() - 3 * 60_000).toISOString()} className="text-sm text-text-muted" />
            <RelativeTime at={new Date(Date.now() - 5 * 3_600_000).toISOString()} className="text-sm text-text-muted" />
            <RelativeTime at="2026-03-01T08:30:00.000Z" className="text-sm text-text-muted" />
          </Row>
        </Specimen>

        <Specimen name="Breadcrumbs, Tree">
          <Breadcrumbs
            crumbs={[
              { id: 'org', label: NAMES.org, render: (classes) => <a href="#org" className={classes}>{NAMES.org}</a> },
              { id: 'project', label: NAMES.project, render: (classes) => <a href="#project" className={classes}>{NAMES.project}</a> },
              { id: 'topic', label: t('dev.sample.nameValue') },
            ]}
          />
          <Breadcrumbs
            crumbs={[
              { id: 'org', label: NAMES.org, go: () => undefined, render: (classes) => <a href="#org" className={classes}>{NAMES.org}</a> },
              { id: 'department', label: NAMES.department, go: () => undefined },
              { id: 'team', label: NAMES.team },
              { id: 'project', label: NAMES.project, go: () => undefined, render: (classes) => <a href="#project" className={classes}>{NAMES.project}</a> },
              { id: 'topic', label: t('dev.sample.nameValue'), render: (classes) => <a href="#topic" className={classes}>{t('dev.sample.nameValue')}</a> },
              { id: 'thread', label: t('dev.sample.thread') },
            ]}
          />
          <div className="rounded-md border border-border bg-bg-subtle py-1">
            <Tree<SampleNode>
              label={t('navigation.tree')}
              rows={rows}
              currentId="main"
              nameOf={(item) => item.name}
              onExpandedChange={(id, open) => {
                const next = new Set(expanded);
                if (open) next.add(id);
                else next.delete(id);
                setExpanded(next);
              }}
              renderRow={sampleRow}
            />
          </div>
          <div className="rounded-md border border-border bg-bg-subtle py-1">
            <Tree<SampleNode>
              label={t('navigation.tree')}
              rows={tickRows}
              nameOf={(item) => item.name}
              onExpandedChange={() => undefined}
              onTick={(row, on) => {
                const next = new Set(ticked);
                // Ticking the project ticks what it holds; a row of its own is changed alone.
                for (const id of row.id === 'web' ? ['fix', 'main'] : [row.id]) {
                  if (on) next.add(id);
                  else next.delete(id);
                }
                setTicked(next);
              }}
              renderRow={sampleRow}
            />
          </div>
        </Specimen>

        <Specimen name="Skeleton, EmptyState, ErrorState">
          <div className="flex flex-col gap-2">
            <Skeleton className="h-4 w-1/3" />
            <Skeleton className="h-3 w-2/3" />
            <SkeletonRows rows={3} />
          </div>
          <EmptyState
            compact
            icon={<Diamond aria-hidden />}
            title={t('dev.sample.empty')}
            description={t('dev.sample.emptyMore')}
            action={
              <Button variant="primary" size="sm" icon={<Plus aria-hidden />}>
                {t('dev.sample.newTopic')}
              </Button>
            }
          />
          <ErrorState compact error={lost} onRetry={() => undefined} />
          <ErrorAlert error={refused} action={<Button size="sm">{t('common.tryAgain')}</Button>} />
        </Specimen>

        <Specimen name="Tabs, Table, Pagination">
          <Tabs
            label={t('dev.sample.tabsLabel')}
            value={tab}
            onValueChange={setTab}
            panelClassName="pt-3"
            tabs={[
              { value: 'overview', label: t('dev.sample.tabOverview'), panel: <p className="text-sm text-text-muted">{t('dev.sample.tabPanel')}</p> },
              {
                value: 'members',
                label: t('dev.sample.tabMembers'),
                count: PEOPLE.length,
                panel: (
                  <div className="flex flex-col gap-2">
                    <Table
                      label={t('dev.sample.tableLabel')}
                      rows={PEOPLE}
                      rowKey={(person) => person.id}
                      rowMenu={{ label: (person) => t('resources.act.menuOf', { name: person.display_name }), entries: () => [{ id: 'copy', label: t('resources.act.copyLink'), onSelect: () => undefined }] }}
                      columns={[
                        { id: 'name', header: t('dev.sample.colName'), cell: (person) => <UserChip user={person} showUsername /> },
                        { id: 'role', header: t('dev.sample.colRole'), from: 'sm', cell: (person) => (person.kind === 'service' ? t('roles.interactor') : t('roles.developer')) },
                        { id: 'joined', header: t('dev.sample.colJoined'), from: 'md', align: 'end', cell: () => <RelativeTime at="2026-10-01T09:00:00.000Z" /> },
                      ]}
                    />
                    <Pagination hasMore loading={false} onMore={() => undefined} />
                  </div>
                ),
              },
              { value: 'activity', label: t('dev.sample.tabActivity'), panel: <p className="text-sm text-text-muted">{t('dev.sample.tabPanel')}</p> },
              { value: 'usage', label: t('dev.sample.tabUsage'), disabled: true, panel: null },
            ]}
          />
        </Specimen>

        <Specimen name="Select, Textarea">
          <Select
            label={t('dev.sample.selectRole')}
            hint={t('dev.sample.selectRoleHint')}
            placeholder={t('dev.sample.selectPlaceholder')}
            value={role}
            onValueChange={setRole}
            options={[
              { value: 'viewer', label: t('roles.viewer'), description: t('dev.sample.roleViewerAbout') },
              { value: 'developer', label: t('roles.developer'), description: t('dev.sample.roleDeveloperAbout') },
              { value: 'admin', label: t('roles.admin'), description: t('dev.sample.roleAdminAbout'), disabled: true },
            ]}
          />
          <Row>
            <Select
              size="sm"
              hideLabel
              label={t('dev.sample.selectRole')}
              value={role}
              onValueChange={setRole}
              options={[
                { value: 'viewer', label: t('roles.viewer') },
                { value: 'developer', label: t('roles.developer') },
              ]}
            />
            <Select
              disabled
              label={t('dev.sample.selectRole')}
              value="viewer"
              onValueChange={() => undefined}
              options={[{ value: 'viewer', label: t('roles.viewer') }]}
            />
          </Row>
          <Textarea
            label={t('dev.sample.description')}
            hint={t('dev.sample.descriptionHint')}
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
          <Textarea label={t('dev.sample.description')} error={t('dev.sample.descriptionError')} defaultValue={t('dev.sample.descriptionValue')} />
        </Specimen>

        <Specimen name="TagChip, UserChip, AvatarStack">
          <Row>
            <TagChip name={NAMES.tagBug} color={2} />
            <TagChip name={NAMES.tagUi} color={5} onRemove={() => undefined} />
          </Row>
          <Row>
            <PersonCard user={ADA} actions={<Button asChild><a href={`/org/acme/settings/audit?actor=${ADA.id}`}><ScrollText aria-hidden />{t('trace.entry')}</a></Button>}><UserChip user={ADA} showUsername /></PersonCard>
            <UserChip user={BOT} />
          </Row>
          <Row>
            <AvatarStack users={[...PEOPLE, BOT]} max={3} />
            <AvatarStack users={[ADA, GRACE]} total={9} max={2} size={28} />
          </Row>
        </Specimen>

        <Specimen name="RadioGroup, PresenceStack, LockBadge, ByteSize, EmojiPicker">
          <RadioGroup<'markdown' | 'html'>
            label={t('documents.export.as')}
            value={format}
            onValueChange={setFormat}
            options={[
              { value: 'markdown', label: t('documents.export.markdown') },
              { value: 'html', label: t('documents.export.html') },
            ]}
          />
          <Row>
            <PresenceStack users={looking([ADA, GRACE])} />
            <PresenceStack users={looking([...PEOPLE, BOT])} without={ADA.id} />
            <LockBadge label={t('documents.access.private')} />
            <ByteSize bytes={2_150_000} />
          </Row>
          <Row>
            <EmojiPicker onPick={setEmoji} />
            <span aria-hidden>{emoji}</span>
          </Row>
        </Specimen>

        <Specimen name="ProvenancePill, QrCode">
          <Row>
            <ProvenancePill kind="default">{t('settings.gallery.default')}</ProvenancePill>
            <ProvenancePill kind="inherited">{t('settings.gallery.inherited')}</ProvenancePill>
            <ProvenancePill kind="here">{t('settings.gallery.here')}</ProvenancePill>
            <ProvenancePill kind="locked">{t('settings.gallery.locked')}</ProvenancePill>
          </Row>
          <Row>
            <QrCode value="otpauth://totp/Orch:ada?secret=JBSWY3DPEHPK3PXP&issuer=Orch" label={t('settings.gallery.qr')} size={128} />
            <p className="max-w-[12rem] text-sm text-text-muted">{t('settings.gallery.qrAbout')}</p>
          </Row>
        </Specimen>

        <Specimen name="PermissionGate, Button blocked, refusal">
          <p className="text-sm text-text-muted">{t('dev.sample.gateOff')}</p>
          <Row>
            <PermissionGate allowed={false} mode="block" reason={denial(t, 'project.manage', NAMES.project, ADA)}>
              <Button icon={<Pencil aria-hidden />}>{t('dev.sample.rename')}</Button>
            </PermissionGate>
            <PermissionGate allowed mode="block">
              <Button icon={<Pencil aria-hidden />}>{t('dev.sample.rename')}</Button>
            </PermissionGate>
          </Row>
          <p className="text-sm text-text-muted">{t('dev.sample.gateNoteLabel')}</p>
          <PermissionGate
            allowed={false}
            mode="note"
            note={<p className="text-sm text-text-muted">{denial(t, 'project.interact', NAMES.project, ADA)}</p>}
          >
            <Button variant="primary">{t('dev.sample.newTopic')}</Button>
          </PermissionGate>
          <p className="text-sm text-text-muted">{t('dev.sample.gateLeftOut')}</p>
          <PermissionGate allowed={false}>
            <Button>{t('dev.sample.rename')}</Button>
          </PermissionGate>
          <p className="text-sm text-text-muted">{t('dev.sample.gateRefused')}</p>
          <ErrorAlert error={denied} />
        </Specimen>

        <Specimen name="ActionIcon, Button (canonical acts)">
          <Row>
            {CRAFT_ACTS.map((action) => <Button key={action} action={action}>{actionName(action)}</Button>)}
          </Row>
        </Specimen>

        <Specimen name="Meter">
          <Meter value={12} label={t('resources.gallery.disk')} />
          <Meter value={84} label={t('resources.gallery.processor')} />
          <Meter value={97} label={t('resources.gallery.memory')} detail={t('resources.gallery.detail')} />
          <p className="text-sm text-text-muted">{t('resources.gallery.inCell')}</p>
          <Meter look="compact" value={64} label={t('resources.gallery.memory')} />
          <p className="text-sm text-text-muted">{t('dev.sample.meterRing')}</p>
          <Row>
            {[42, 86, 97].map((percent) => (
              <Meter key={percent} look="ring" value={percent} label={t('thread.agent.context', { percent })} />
            ))}
          </Row>
          <p className="text-sm text-text-muted">{t('dev.sample.meterProgress')}</p>
          <Meter look="progress" value={45} label={t('thread.lock.progress')} />
        </Specimen>

        <Specimen name="RadioGroup">
          <ChoiceSample />
        </Specimen>

        <Specimen name="HelpHint">
          <HintSample />
        </Specimen>

        <Specimen name="FieldForm">
          <DeclaredFieldsSample />
        </Specimen>

        <Specimen name="CodeView, Sparkline" wide>
          <div className="h-48 overflow-hidden rounded-md border border-border">
            <CodeView
              className="h-full"
              text={SAMPLE_CODE}
              version="sample"
              language={SAMPLE_LANGUAGE?.load ?? null}
              languageKey={SAMPLE_LANGUAGE?.id ?? null}
              label={t('files.code.label', { name: 'app.ts' })}
            />
          </div>
          <Sparkline
            label={t('inspector.resources.chartCpu')}
            series={[{ values: [12, 18, 30, 24, 40, 62, 55, 48, 70, 66, 52, 44] }]}
            limit={80}
            max={100}
          />
        </Specimen>

        <Specimen name="StatusBar" wide>
          <div className="overflow-hidden rounded-md border border-border">
            <StatusBar end={<StatusPart onOpen={() => undefined}>{t('views.help.title')}</StatusPart>}>
              <StatusPart tone="warning" hint={t('workbench.status.slowHint')}>
                {t('workbench.status.slow', { ms: 320 })}
              </StatusPart>
              <StatusPart icon={<LoaderCircle size={12} className="animate-spin" aria-hidden />} onOpen={() => undefined}>
                {t('dev.sample.statusThread')}
              </StatusPart>
              <StatusPart icon={<GitBranch size={12} aria-hidden />} onOpen={() => undefined}>
                {NAMES.branch}
              </StatusPart>
            </StatusBar>
          </div>
        </Specimen>

        <AccessControls />

        <Specimen name="TimeSeriesChart" wide>
          <TimeSeriesChart
            label={t('admin.overview.charts.turns')}
            lines={chartLines}
            format={(value) => formatNumber(value, locale)}
            from={CHART_FROM}
            to={CHART_TO}
            words={{
              table: t('admin.overview.chartTable'),
              time: t('admin.overview.chartTime'),
              now: t('admin.overview.chartNow'),
              peak: t('admin.overview.chartPeak'),
              keys: t('admin.overview.chartKeys'),
            }}
            empty={t('admin.overview.nothingStored')}
          />
        </Specimen>

        <section aria-label={OVERLAYS} className="flex flex-col gap-2">
          <h2 className="font-mono text-sm font-medium text-text-muted">{OVERLAYS}</h2>
          <div className="flex flex-col gap-4 rounded-lg border border-border bg-bg p-4">
            <p className="text-sm text-text-muted">{t('dev.overlaysNote')}</p>
            <Row>
              <Menu label={t('dev.sample.menu')} entries={entries} trigger={<Button>{t('dev.sample.menu')}</Button>} />
              <ContextMenu label={t('dev.sample.menu')} entries={entries}>
                <div
                  tabIndex={0}
                  role="group"
                  aria-label={t('dev.sample.context')}
                  className="flex h-(--control-md) items-center rounded-md border border-dashed border-border-strong px-3 text-sm text-text-muted"
                >
                  {t('dev.sample.context')}
                </div>
              </ContextMenu>
              <Popover label={t('dev.sample.popover')} trigger={<Button>{t('dev.sample.popover')}</Button>}>
                <p className="text-sm">{t('dev.sample.popoverBody')}</p>
              </Popover>
              <Button onClick={() => setDialog(true)}>{t('dev.sample.dialog')}</Button>
              <Button onClick={() => setConfirm(true)}>{t('dev.sample.confirmLabel')}</Button>
              <Button onClick={() => setDrawer(true)}>{t('dev.sample.drawer')}</Button>
              <Button onClick={() => toast({ tone: 'success', title: t('dev.sample.toastDone') })}>{t('dev.sample.toastDone')}</Button>
              <Button icon={<RotateCcw aria-hidden />} onClick={() => toast({ tone: 'success', title: t('dev.sample.toastDone'), action: { label: t('day100.batch.undo'), icon: <RotateCcw aria-hidden />, run: () => undefined } })}>{t('day100.batch.undo')}</Button>
              <Button
                onClick={() =>
                  toast({
                    tone: 'danger',
                    title: t('dev.sample.toastFailed'),
                    description: t('dev.sample.toastFailedMore'),
                    action: { label: t('common.tryAgain'), run: () => undefined },
                  })
                }
              >
                {t('dev.sample.toastFailed')}
              </Button>
              <Button variant="ghost" onClick={() => void router.navigate({ to: '/login' })}>
                {t('auth.links.signIn')}
              </Button>
            </Row>
          </div>
        </section>
      </main>

      <Dialog
        open={dialog}
        onOpenChange={setDialog}
        size="sm"
        title={t('dev.sample.dialog')}
        description={t('dev.sample.dialogLead')}
        footer={
          <>
            <Button onClick={() => setDialog(false)}>{t('common.cancel')}</Button>
            <Button variant="primary" onClick={() => setDialog(false)}>
              {t('dev.sample.rename')}
            </Button>
          </>
        }
      >
        <Input label={t('dev.sample.name')} defaultValue={t('dev.sample.nameValue')} />
      </Dialog>
      <ConfirmDialog
        open={confirm}
        onOpenChange={setConfirm}
        title={t('dev.sample.confirm')}
        confirmLabel={t('dev.sample.confirmLabel')}
        danger
        onConfirm={() => setConfirm(false)}
        override={{ hint: t('dev.sample.override'), additions: [t('dev.sample.overrideAddition')] }}
      >
        {t('dev.sample.confirmBody')}
      </ConfirmDialog>
      <Drawer side="right" open={drawer} onOpenChange={setDrawer} title={t('dev.sample.drawer')}>
        <EmptyState compact icon={<Diamond aria-hidden />} title={t('dev.sample.empty')} description={t('dev.sample.emptyMore')} />
      </Drawer>
    </div>
  );
}

/** What the code view's specimen shows, and the language it is read in. */
const SAMPLE_CODE = `import { greet } from './util';

/** The entry point of the page. */
export function main(name: string): string {
  const message = greet(name);
  return message;
}
`;
const SAMPLE_LANGUAGE = languageOf('app.ts');

/** The name of the section whose controls open above the page: a word of the code, the same in every language. */
const OVERLAYS = 'Dialog, ConfirmDialog, Drawer, Popover, Menu, ContextMenu, Toast';
const CRAFT_ACTS = ['save', 'move', 'makePrivate', 'inheritAccess', 'comments', 'cancel'] as const;

const KIND_ICONS = { org: Folder, project: Folder, topic: Diamond, thread: MessageSquareText };

/** The kinds of place an icon is drawn for, each in the colour of its kind: words of the code, the same in every language. */
const PLACE_KINDS = ['org', 'project', 'folder', 'topic'] as const;

function sampleRow(row: TreeRow<SampleNode>): ReactNode {
  const Icon = KIND_ICONS[row.item.kind];
  return (
    <span className="flex min-w-0 flex-1 items-center gap-2">
      {row.item.kind === 'thread' ? <StateIcon state="running" /> : <Icon className="text-text-subtle" aria-hidden />}
      <span className="truncate">{row.item.name}</span>
    </span>
  );
}

/** The people behind a role in the samples: two pages, so that "Show more" has something to show. */
const SAMPLE_PEOPLE: PeopleSource = {
  queryKey: ['dev', 'holder-people'],
  initialPageParam: null,
  queryFn: ({ pageParam }) => {
    const second = pageParam === 'more';
    return Promise.resolve({ items: second ? [MING, SALLY] : [ADA, GRACE], next_cursor: second ? null : 'more' });
  },
  getNextPageParam: (last) => last.next_cursor,
};

const SAMPLE_HISTORY: readonly HistoryLine[] = [
  { id: 'h1', at: '2026-10-11T10:00:00.000Z', actor: ADA, action: 'permission.added', text: `${NAMES.ada} named ${NAMES.grace} in Can interact`, about: null },
  { id: 'h2', at: '2026-10-10T09:00:00.000Z', actor: GRACE, action: 'node.created', text: `${NAMES.grace} made the topic ${NAMES.project}`, about: null },
];

/** What a page is drawn from: the field that picks whom to name, the chip of a role with its people on request, the lock, and the lines of a history. */
function AccessControls() {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [picked, setPicked] = useState<string | null>(null);
  const groups: ComboboxGroup<string>[] = [
    { id: 'people', label: t('access.candidates.people'), options: PEOPLE.map((who) => ({ value: who.id, label: who.display_name, text: who.display_name, description: `@${who.username}`, disabled: who === BOT ? t('access.gallery.off') : false })) },
    { id: 'roles', label: t('access.candidates.roles'), options: [{ value: 'developer', label: t('access.holder.roles.developer'), text: t('access.holder.roles.developer'), icon: <ShieldCheck aria-hidden /> }] },
  ];
  return (
    <Specimen name="Combobox, HolderChip, LockBadge, HistoryLines">
      <Combobox
        label={t('access.gallery.field')}
        placeholder={t('access.add.placeholder')}
        query={query}
        onQueryChange={setQuery}
        groups={groups}
        onSelect={(option) => setPicked(option.text)}
        hint={picked === null ? undefined : t('access.gallery.picked', { name: picked })}
      />
      <Row>
        <HolderChip name={t('access.holder.roles.developer')} count={14} icon={<ShieldCheck aria-hidden />} people={SAMPLE_PEOPLE} />
        <HolderChip name={NAMES.team} count={3} icon={<Users aria-hidden />} inherited />
        <HolderChip name={t('access.holder.roles.admin')} icon={<ShieldCheck aria-hidden />} pending />
      </Row>
      <Row>
        <LockBadge>{t('access.cell.locked', { place: NAMES.org })}</LockBadge>
        <LockBadge onOpen={() => undefined}>{t('access.cell.locked', { place: NAMES.org })}</LockBadge>
        <LockBadge label={t('thread.access.restricted')} />
      </Row>
      <HistoryLines lines={SAMPLE_HISTORY} />
    </Specimen>
  );
}
