// @vitest-environment node
/** Shared module doubles keep exports added by the code their screens render. */
import { readFileSync, readdirSync } from 'node:fs';
import { dirname, relative, resolve } from 'node:path';
import ts from 'typescript';
import { expect, it } from 'vitest';

const application = resolve(import.meta.dirname, '../../src');
/** Only module-load suppliers and common frame dependencies belong here; screen replacements stay local. */
const shared = new Map<string, string>([
  ['@/api/resources', 'Resource and composer screens read providerModelsQuery, agentQuery and agentsQuery through shared controls.'],
  ['@/api/environments', 'api/start.ts builds managedEnvironmentsQuery from environmentsKey at module load.'],
  ['@/features/resources/harnesses', 'Shared resource and binding controls read useHarnessNames while rendering.'],
  ['@/features/workbench/Narrow', 'The shell reads shape and compact-control hooks in every frame.'],
  ['@/features/change/target', 'Shared composer and resource actions read openChange.'],
  ['@/features/composer/runsWith', 'Shared composer controls read useRunsWith.'],
  ['@tanstack/react-router', 'The common TestRouter wrapper and shell read router constructors, providers and Link.'],
  ['@/api/acts', 'realtime/live.ts initializes the exported follow function with followWith at load.'],
  ['@/api/auth', 'shell/admission.ts spreads metaQuery while initializing admissionMetaQuery.'],
  ['@/api/client', 'Shell.tsx and realtime/live.ts register client session and action listeners at load.'],
  ['@/api/duplicates', 'api/resourceHome.ts reads duplicateObjects while creating its module-level catalogue.'],
  ['@/api/exports', 'api/trace.ts reads jobQuery while initializing its exported job reader.'],
  ['@/api/notifications', 'api/away.ts reads notificationPrefsKey while initializing its query.'],
  ['@/api/query', 'realtime/live.ts installs query feeds against queryClient at load.'],
  ['@/api/workspace', 'realtime/stores/environment.ts passes listDirectory to DirectoryWindows at load.'],
  ['@/features/files/lazy', 'ChangeHost.tsx calls lazyPart while creating its dialog parts at load.'],
  ['@/features/navigation/Resources', 'shell/Rail.tsx reads RESOURCES_VIEW while building its view catalogue.'],
  ['@/features/resources/ResourceCreation', 'navigation/Resources.tsx initializes its exported add menu from ResourceAddMenu.'],
  ['@/features/workbench/store', 'CloseViews.tsx and commentDrafts.ts subscribe to useWorkbench at module load.'],
  ['@/features/workbench/views', 'Chat and document registration modules call registerView at load.'],
  ['@/lib/time', 'Usage series and forecast modules initialize intervals from the shared time constants.'],
  ['@/realtime/queryFeeds', 'realtime/live.ts calls installQueryFeeds at module load.'],
  ['@/realtime/stores/thread', 'realtime/live.ts calls setGapHandler at module load.'],
  ['@/realtime/stores/user', 'Navigation attention, reactions and live feeds register onUserEvent at load.'],
  ['@/features/access/AccessDialog', 'The common shell mounts AccessHost.'],
  ['@/features/access/AdminMode', 'The common ScreenTitleBar mounts AdminModeControl.'],
  ['@/features/browser/View', 'The common frame services call useBrowserRegistration.'],
  ['@/features/export/ExportHost', 'The common shell mounts ExportHost.'],
  ['@/features/help/Tutorial', 'The common frame services mount TutorialHost.'],
  ['@/features/navigation/NodeDialogs', 'The common shell mounts NodeDialogHost.'],
  ['@/features/vnc/View', 'The common frame services call useVncRegistration.'],
  ['@/features/workbench/choiceHelp', 'The common title bar reads useWorkbenchChoiceHelp.'],
  ['@/features/workbench/openView', 'The common shell holds view reveals at frame admission.'],
  ['@/features/workbench/viewOpening', 'The common shell clears view openings at reader changes.'],
  ['@/realtime/stores/chat', 'The common frame services call useChatActivity.'],
  ['@/realtime/stores/tree', 'The common Breadcrumbs reads useNodePath and useTreeStore.'],
  ['@/api/nodes', 'The common Breadcrumbs reads nodeQuery even before its heading is ready.'],
  ['@/api/adminMode', 'The common title-bar account control reads adminModeQuery.'],
  ['@/shell/Banners', 'The common shell mounts Banners.'],
  ['@/shell/FrameReadiness', 'The common shell supplies FrameReadiness and its shared controls read useFrameReady.'],
  ['@/shell/Rail', 'The common shell mounts Rail.'],
  ['@/shell/TitleBar', 'The common title bar mounts ScreenTitleBar.'],
  ['@/shell/bootstrap', 'The common shell reads useShellBootstrap at admission.'],
  ['@/shell/keys', 'The common title bar reads useKeyChoices.'],
  ['@/shell/org', 'The common shell reads and clears the selected organization.'],
  ['@/shell/screen', 'The common Breadcrumbs reads useCurrentHeading.'],
  ['@/ui/Toast', 'The common component wrapper and shell mount Toaster.'],
  ['@/ui/Tooltip', 'The common component wrapper mounts TooltipProvider.'],
  ['@/features/ask/open', 'shell/Shortcuts.tsx reads openAsk in the common frame.'],
  ['@/features/inbox/Counters', 'shell/Rail.tsx reads CountBadge, useCounterWords, useInboxCounters in the common frame.'],
  ['@/features/navigation/counts', 'features/navigation/Sidebar.tsx reads useUnreadChats in the common frame.'],
  ['@/features/navigation/currentOrg', 'features/navigation/Sidebar.tsx reads useNavigationOrg in the common frame.'],
  ['@/features/navigation/OrgSwitcher', 'features/navigation/Sidebar.tsx reads OrgSwitcher in the common frame.'],
  ['@/features/navigation/placeLabels', 'features/workbench/PageRouting.tsx reads useNodeLabel in the common frame.'],
  ['@/routes/-access', 'shell/Rail.tsx reads leave in the common frame.'],
  ['@/features/help/KeyList', 'shell/Shortcuts.tsx reads KeyList in the common frame.'],
  ['@/features/help/open', 'shell/Shortcuts.tsx reads useHelpOpener in the common frame.'],
  ['@/features/inspector/reviewTarget', 'shell/Shortcuts.tsx reads openThreadChanges in the common frame.'],
  ['@/api/access', 'shell/bootstrap.ts reads permissionsKey in the common frame.'],
  ['@/api/away', 'shell/bootstrap.ts reads outsideQuery in the common frame.'],
  ['@/api/chats', 'features/navigation/Sidebar.tsx reads orgChatsQuery in the common frame.'],
  ['@/realtime/live', 'shell/RouteContext.tsx reads useDocumentFeed, useThreadFeed in the common frame.'],
  ['@/api/personal', 'shell/bootstrap.ts reads favoritesKey, meStateKey in the common frame.'],
  ['@/features/access/ParticipationFilter', 'features/navigation/Sidebar.tsx reads useParticipationCopy in the common frame.'],
  ['@/api/threads', 'features/navigation/Sidebar.tsx reads markThreadRead in the common frame.'],
  ['@/features/chat/acts', 'features/navigation/Sidebar.tsx reads readChat in the common frame.'],
  ['@/features/chat/open', 'features/navigation/Sidebar.tsx reads useOpenPath in the common frame.'],
  ['@/features/navigation/DocMenu', 'features/navigation/Sidebar.tsx reads DocContextMenu, DocMenuButton in the common frame.'],
  ['@/features/navigation/favorites', 'features/navigation/Sidebar.tsx reads useFavorites in the common frame.'],
  ['@/features/navigation/NodeMenu', 'features/navigation/Sidebar.tsx reads NodeContextMenu, NodeMakeButton, NodeMenuButton in the common frame.'],
  ['@/features/navigation/places', 'shell/RouteContext.tsx reads isFolderPath in the common frame.'],
  ['@/lib/viewChoices', 'features/navigation/Sidebar.tsx reads readViewChoice, writeViewChoice, useViewChoice in the common frame.'],
  ['@/lib/title', 'features/workbench/PageRouting.tsx reads usePageTitle in the common frame.'],
  ['@/realtime/threadReadContext', 'shell/RouteContext.tsx reads useThreadReadRef in the common frame.'],
  ['@/api/threadRead', 'shell/RouteContext.tsx reads selectThreadContinuation in the common frame.'],
  ['@/features/workbench/settingsTabTitle', 'shell/RouteContext.tsx reads useSettingsTabTitle in the common frame.'],
  ['@/features/usage/Board', 'shell/RouteContext.tsx reads useUsageBoardParams, useWeeklyUsageParams in the common frame.'],
]);

function moduleName(name: string, file: string): string {
  if (name.startsWith('.')) {
    const path = relative(application, resolve(dirname(file), name));
    if (!path.startsWith('..')) return `@/${path.replaceAll('\\', '/').replace(/\.tsx?$/, '')}`;
  }
  return name.replace(/\.tsx?$/, '');
}

function unwrap(expression: ts.Expression): ts.Expression {
  return ts.isParenthesizedExpression(expression) || ts.isAsExpression(expression)
    ? unwrap(expression.expression) : expression;
}

/** A real-module spread must come from the mock's own loader or its exact importActual. */
export function closedModuleDoubles(text: string, file = resolve(application, 'fixture.test.tsx')): { line: number; module: string }[] {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const closed: { line: number; module: string }[] = [];
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)
      && node.expression.expression.getText(source) === 'vi'
      && ['mock', 'doMock'].includes(node.expression.name.text)) {
      const argument = node.arguments[0];
      const name = argument !== undefined && ts.isStringLiteral(argument) ? argument.text
        : argument !== undefined && ts.isCallExpression(argument)
          && argument.expression.kind === ts.SyntaxKind.ImportKeyword
          && argument.arguments[0] !== undefined && ts.isStringLiteral(argument.arguments[0])
          ? argument.arguments[0].text : undefined;
      const factory = node.arguments[1];
      if (name !== undefined && factory !== undefined && (name.startsWith('@/') || name.startsWith('.') || name === '@tanstack/react-router')
        && (ts.isArrowFunction(factory) || ts.isFunctionExpression(factory))) {
        const parameter = factory.parameters[0]?.name;
        const loader = parameter !== undefined && ts.isIdentifier(parameter) ? parameter.text : undefined;
        const loaded = (expression: ts.Expression): boolean => {
          const value = unwrap(expression);
          if (!ts.isAwaitExpression(value) || !ts.isCallExpression(value.expression)) return false;
          const call = value.expression;
          if (ts.isIdentifier(call.expression) && call.expression.text === loader) return true;
          const imported = call.arguments[0];
          return ts.isPropertyAccessExpression(call.expression)
            && call.expression.expression.getText(source) === 'vi' && call.expression.name.text === 'importActual'
            && imported !== undefined && ts.isStringLiteral(imported)
            && moduleName(imported.text, file) === moduleName(name, file);
        };
        const originals = new Set<string>();
        const returns: ts.Expression[] = [];
        if (ts.isBlock(factory.body)) {
          for (const statement of factory.body.statements) {
            if (ts.isVariableStatement(statement)) for (const declaration of statement.declarationList.declarations) {
              if (ts.isIdentifier(declaration.name) && declaration.initializer !== undefined && loaded(declaration.initializer)) originals.add(declaration.name.text);
            }
            if (ts.isReturnStatement(statement) && statement.expression !== undefined) returns.push(statement.expression);
          }
        } else returns.push(factory.body);
        const spreadsOriginal = (expression: ts.Expression) => {
          const value = unwrap(expression);
          return ts.isObjectLiteralExpression(value) && value.properties.some((property) => ts.isSpreadAssignment(property)
            && (loaded(property.expression) || (ts.isIdentifier(property.expression) && originals.has(property.expression.text))));
        };
        if (returns.length === 0 || !returns.every(spreadsOriginal)) closed.push({
          line: source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1,
          module: moduleName(name, file),
        });
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return closed;
}

it('recognizes closed application doubles without accepting an unrelated object spread', () => {
  expect(closedModuleDoubles("vi.mock('@/features/example', () => ({ useOldName: () => 1 }));")).toEqual([{ line: 1, module: '@/features/example' }]);
  expect(closedModuleDoubles("vi.mock('@/features/example', () => ({ ...fixture, useOldName: () => 1 }));")).toHaveLength(1);
  expect(closedModuleDoubles("vi.mock('@/features/example', async () => ({ ...await vi.importActual('@/features/other') }));")).toHaveLength(1);
  expect(closedModuleDoubles("vi.doMock(import('@/features/example'), () => ({ useOldName: () => 1 }));")).toHaveLength(1);
});

it('accepts real module spreads while keeping each test override', () => {
  expect(closedModuleDoubles("vi.mock('@/features/example', async (load) => ({ ...await load<object>(), useOldName: () => 1 }));")).toEqual([]);
  expect(closedModuleDoubles("vi.mock('@/features/example', async (load) => { const actual = await load<object>(); return { ...actual, useOldName: () => 1 }; });")).toEqual([]);
  expect(closedModuleDoubles("vi.mock('@/features/example', async () => ({ ...await vi.importActual('@/features/example'), useOldName: () => 1 }));")).toEqual([]);
  expect(closedModuleDoubles("vi.mock('external-library', () => ({ oldName: 1 }));")).toEqual([]);
});

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => entry.isDirectory()
    ? sources(resolve(directory, entry.name)) : /\.test\.tsx?$/.test(entry.name) ? [resolve(directory, entry.name)] : []);
}

// Source inventory is fixture preparation. Parsing an unrelated test file does
// not belong to a timed assertion about whether the resulting list is empty.
const tests = resolve(import.meta.dirname, '..');
const closed = sources(tests).flatMap((file) => {
  const text = readFileSync(file, 'utf8');
  const relevant = [...text.matchAll(/\bvi\.(?:mock|doMock)\s*\(\s*(?:import\s*\(\s*)?['"]([^'"]+)['"]/g)]
    .some((match) => shared.has(moduleName(match[1] ?? '', file)));
  return relevant ? closedModuleDoubles(text, file).filter((mock) => shared.has(mock.module))
    .map((mock) => `${relative(tests, file)}:${mock.line} ${mock.module} must spread its real module: ${shared.get(mock.module) ?? 'shared frame dependency'}`) : [];
});

it('keeps the real exports in every double of the shared screen modules', () => {
  expect(closed).toEqual([]);
});
