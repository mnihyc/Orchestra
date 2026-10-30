/** The views of a thread beside its conversation, each of which can go into any area of the workbench.
 *
 * The workbench offers every view of a thread from the start and shows a note in each until the part that
 * builds it registers the real one under the same name. This is where the views of the workspace, the
 * runtime and the conversation's outline are registered. Each body is fetched when its view is first
 * drawn, three times over if need be, and a body that failed to arrive is made again when it is asked for
 * again, so that "Try again" in the view that stopped means something.
 */
import { Bot, File as FileIcon, ChartNoAxesColumn, History, MessageSquareDashed, Users, BookOpen, Bug, ScrollText, Network } from 'lucide-react';

import type { ComponentType } from 'react';

import { lazyPart } from '@/features/files/lazy';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { ACTIONS } from '@/ui/actions';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { ChangesSummary, EnvironmentSummary, PeopleSummary, ProcessesSummary } from '@/features/workbench/Summaries';
import { Hint } from '@/features/help/Hint';

const THREAD = ['thread'] as const;
const THREAD_OR_TOPIC = ['topic', 'thread'] as const;

let registered = false;

function LinksHelp() { return <Hint id="document-links" popover />; }

/** Registers the views of this part, once, in place of the notes that said they were not built yet. */
export function registerInspectorViews(): void {
  if (registered) return;
  registered = true;

  registerView({
    name: 'files',
    title: 'views.files.title',
    about: 'views.files.about',
    icon: ACTIONS.files.Icon,
    screens: THREAD_OR_TOPIC,
    area: 'right',
    body: lazyPart<ViewProps>(() => import('./Files').then((module) => ({ default: module.FilesView }))),
  });
  // The views of a thread's right bar, each in the place of its note. Their names, titles and kinds of screen are the ones the notes had.
  const part = (load: () => Promise<{ default: ComponentType<ViewProps> }>) => lazyPart<ViewProps>(load);
  registerView({ name: 'subagent', title: 'timeline.subagent.panelTitle', about: 'timeline.subagent.panelAbout', icon: Bot, screens: THREAD, area: 'right', container: 'conversation', order: 3,
    available: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === 'subagent' && tab.of?.item !== undefined),
    body: part(() => import('../timeline/SubagentLane').then((module) => ({ default: module.SubagentPanel }))) });
  registerView({ name: 'side-thread', title: 'side.panelTitle', about: 'side.panelAbout', icon: MessageSquareDashed, screens: THREAD, area: 'right', container: 'conversation', order: 4,
    available: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === 'side-thread' && tab.of?.side !== undefined),
    body: part(() => import('../timeline/SideThreadLane').then((module) => ({ default: module.SideThreadPanel }))) });
  registerView({ name: 'history', title: 'threadViews.history.title', about: 'threadViews.history.about', icon: History, screens: THREAD, area: 'right', body: part(() => import('./History').then((module) => ({ default: module.HistoryView }))) });
  registerView({ name: 'people', title: 'threadViews.people.title', about: 'threadViews.people.about', icon: Users, screens: THREAD, area: 'right', container: 'team', order: 2, summary: PeopleSummary, body: part(() => import('./People').then((module) => ({ default: module.PeopleView }))) });
  registerView({ name: 'knowledge', title: 'threadViews.knowledge.title', about: 'threadViews.knowledge.about', icon: BookOpen, screens: THREAD, area: 'right', body: part(() => import('./Knowledge').then((module) => ({ default: module.KnowledgeView }))) });
  // What Orch gave the agent, exactly as it was sent: read by whoever reads the thread.
  registerView({ name: 'given', title: 'threadViews.given.title', about: 'threadViews.given.about', icon: ScrollText, screens: THREAD, area: 'right', body: part(() => import('./Given').then((module) => ({ default: module.GivenView }))) });
  registerView({ name: 'diagnostics', title: 'threadViews.diagnostics.title', about: 'threadViews.diagnostics.about', icon: Bug, screens: THREAD, area: 'right', body: part(() => import('./Diagnostics').then((module) => ({ default: module.DiagnosticsView }))) });
  registerView({ name: 'usage', title: 'providerPlan.threadTitle', about: 'providerPlan.ledgerLead', icon: ChartNoAxesColumn, screens: THREAD, area: 'right', body: part(() => import('../thread/UsageView').then((module) => ({ default: module.ThreadUsageView }))) });
  registerView({ name: 'outline', title: 'views.outline.title', about: 'views.outline.about', icon: ACTIONS.outline.Icon, screens: ['thread', 'document'], area: 'right', body: part(() => import('./Outline').then((module) => ({ default: module.OutlineView }))) });
  registerView({ name: 'links', title: 'links.title', about: 'links.about', icon: Network, screens: ['document'], area: 'right', container: 'document', order: 2, actions: LinksHelp, body: part(() => import('../links/Links').then((module) => ({ default: module.LinksView }))) });
  registerView({ name: 'changes', title: 'views.changes.title', about: 'views.changes.about', icon: ACTIONS.changes.Icon, screens: THREAD, area: 'right', container: 'code', order: 0, summary: ChangesSummary, body: part(() => import('./Changes').then((module) => ({ default: module.ChangesView }))) });
  registerView({ name: 'git', title: 'views.git.title', about: 'views.git.about', icon: ACTIONS.git.Icon, screens: THREAD, area: 'right', body: part(() => import('./Git').then((module) => ({ default: module.GitView }))) });
  registerView({ name: 'processes', title: 'views.processes.title', about: 'views.processes.about', icon: ACTIONS.processes.Icon, screens: THREAD, area: 'right', container: 'environment', order: 1, summary: ProcessesSummary, body: part(() => import('./Processes').then((module) => ({ default: module.ProcessesView }))) });
  registerView({ name: 'resources', title: 'views.resources.title', about: 'views.resources.about', icon: ACTIONS.resources.Icon, screens: THREAD, area: 'right', body: part(() => import('./Resources').then((module) => ({ default: module.ResourcesView }))) });
  registerView({ name: 'terminal', title: 'views.terminal.title', about: 'views.terminal.about', icon: ACTIONS.terminal.Icon, screens: THREAD, area: 'bottom', body: part(() => import('./Terminal').then((module) => ({ default: module.TerminalView }))) });
  registerView({ name: 'environment', title: 'views.environment.title', about: 'views.environment.about', icon: ACTIONS.environment.Icon, screens: THREAD_OR_TOPIC, area: 'right', container: 'environment', order: 0, summary: EnvironmentSummary, body: part(() => import('./Environment').then((module) => ({ default: module.EnvironmentView }))) });
  registerView({ name: 'parameters', title: 'views.parameters.title', about: 'views.parameters.about', icon: ACTIONS.parameters.Icon, screens: THREAD, area: 'right', body: part(() => import('./Parameters').then((module) => ({ default: module.ParametersView }))) });
  registerView({
    name: 'file',
    role: 'content',
    available: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === 'file' && tab.of?.path !== undefined),
    title: 'files.fileView.title',
    about: 'files.fileView.about',
    icon: FileIcon,
    screens: ['thread', 'page'],
    area: 'main',
    body: lazyPart<ViewProps>(() => import('../files/FileTabs').then((module) => ({ default: module.FileTab }))),
  });
  registerView({
    name: 'difference',
    role: 'content',
    available: () => allTabs(useWorkbench.getState().layout).some((tab) => tab.view === 'difference' && tab.of?.path !== undefined),
    title: 'files.differenceView.title',
    about: 'files.differenceView.about',
    icon: ACTIONS.changes.Icon,
    screens: THREAD,
    area: 'main',
    body: lazyPart<ViewProps>(() => import('../files/viewers/Difference').then((module) => ({ default: module.DifferenceTab }))),
  });
}
