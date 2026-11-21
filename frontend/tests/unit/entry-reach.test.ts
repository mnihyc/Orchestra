/** The code the page loads first holds the shell and the screens' addresses, and none of what loads when it is first used. */
import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import ts from 'typescript';

import { entryGraph, wayTo } from './entry-graph';
import { resolveImport, staticSpecifiers } from '../../build/module-graph';

/** Loaded when first used: the code editor, the difference view, the settings registry, and what the workbench's screens stand on. */
const MODULES = [
  'ui/CodeView.tsx',
  'ui/DiffView.tsx',
  'generated/settings-registry.ts',
  'features/finder/Finder.tsx',
  'features/access/AccessDialogBody.tsx',
  'features/workbench/Board.tsx',
  'features/thread/Thread.tsx',
  'features/composer/Composer.tsx',
];
/** The libraries of the editor, the terminal, the charts, the PDF reader, the documents' editor, the diagrams and the remote screen. */
const PACKAGES = [/^@codemirror\//, /^@lezer\//, /^@xterm\//, /^pdfjs-dist$/, /^uplot$/, /^d3-force$/, /^katex$/, /^mermaid$/, /^cytoscape/, /^@tiptap\//, /^@novnc\//, /^@vedivad\//];

it('keeps session and vocabulary waits out of module evaluation so shared screen imports can be packed', () => {
  const source = ts.createSourceFile('main.tsx', readFileSync(resolve(process.cwd(), 'src/main.tsx'), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const waits: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isFunctionLike(node)) return;
    if (ts.isAwaitExpression(node) || ts.isForOfStatement(node) && node.awaitModifier !== undefined) waits.push(node.getText(source));
    ts.forEachChild(node, visit);
  };
  visit(source);
  expect(waits).toEqual([]);
});

it('draws the first shell before fetching the router, feature registrations and live stores', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/main.tsx');
  const later = ['app.tsx', 'shell/startup.ts', 'realtime/live.ts', 'features/workbench/store.ts', 'api/client.ts', 'i18n/index.ts'];
  expect(later.filter((module) => graph.modules.has(module)).map((module) => wayTo(graph, module).join(' -> '))).toEqual([]);
});

it('holds none of the editor, the difference view, the charts, the PDF reader, the terminal or the settings registry', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/main.tsx');
  const reached = MODULES.filter((module) => graph.modules.has(module)).map((module) => wayTo(graph, module).join(' -> '));
  expect(reached, 'modules that load on first use are reached by static imports').toEqual([]);
  const libraries = [...graph.packages].filter(([name]) => PACKAGES.some((pattern) => pattern.test(name))).map(([name, from]) => `${name} (from ${from})`);
  expect(libraries, 'libraries that load on first use are reached by static imports').toEqual([]);
});

it('uses the router’s existing page metadata without another script exchange before a page body', () => {
  const manifest = ts.createSourceFile('pages.ts', readFileSync(resolve(process.cwd(), 'src/features/workbench/pages.ts'), 'utf8'), ts.ScriptTarget.Latest, true);
  const imports: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && node.expression.kind === ts.SyntaxKind.ImportKeyword
      && node.arguments[0] !== undefined && ts.isStringLiteralLike(node.arguments[0])) imports.push(node.arguments[0].text);
    ts.forEachChild(node, visit);
  };
  visit(manifest);
  expect(imports.filter((path) => path.startsWith('@/routes/'))).toEqual([]);
  const graph = ts.createSourceFile('startup.ts', readFileSync(resolve(process.cwd(), 'src/shell/startup.ts'), 'utf8'), ts.ScriptTarget.Latest, true);
  imports.length = 0; visit(graph);
  expect(imports).not.toContain('@/features/workbench/pages');
});

it('shows a pending schedule without fetching its settings editor or complete management screen', { timeout: 120_000 }, async () => {
  const file = resolve(process.cwd(), 'src/features/thread/Thread.tsx');
  const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const owner = source.statements.find((node): node is ts.ImportDeclaration => ts.isImportDeclaration(node)
    && node.importClause?.namedBindings !== undefined && ts.isNamedImports(node.importClause.namedBindings)
    && node.importClause.namedBindings.elements.some((item) => item.name.text === 'PendingAgentSchedules'));
  if (owner === undefined || !ts.isStringLiteralLike(owner.moduleSpecifier)) throw new Error('The thread must still show its pending schedules.');
  const target = resolveImport(resolve(process.cwd(), 'src'), file, owner.moduleSpecifier.text);
  if (target === null || !('file' in target)) throw new Error('The pending schedule module must be present.');
  const graph = await entryGraph(relative(process.cwd(), target.file));
  const deferred = ['features/schedules/Schedules.tsx', 'features/schedules/Editor.tsx', 'features/settings/Editor.tsx'];
  expect(deferred.filter((module) => graph.modules.has(module)).map((module) => wayTo(graph, module).join(' -> '))).toEqual([]);
});

it('opens the workbench frame without loading optional views and command palette registrations', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/shell/Shell.tsx');
  for (const path of ['features/help/HelpView.tsx', 'features/help/Tutorial.tsx', 'features/finder/shellCommands.tsx', 'features/chat/ChatPanel.tsx', 'features/navigation/Sidebar.tsx', 'features/start/BindingDialog.tsx', 'features/navigation/RecentlyDeleted.tsx', 'features/navigation/NodeDialogBody.tsx']) {
    expect(graph.modules.has(path), wayTo(graph, path).join(' -> ')).toBe(false);
  }
});

it('opens the thread frame without fetching the usage board of a project or topic', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/shell/Shell.tsx');
  const board = 'features/usage/Board.tsx';
  expect(graph.modules.has(board), wayTo(graph, board).join(' -> ')).toBe(false);
});

it('shows the workbench’s view chooser button before downloading the command palette', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/shell/Shell.tsx');
  expect(graph.modules.has('ui/Palette.tsx'), wayTo(graph, 'ui/Palette.tsx').join(' -> ')).toBe(false);
  expect(graph.packages.has('cmdk')).toBe(false);
});

it('opens the conversation header without importing the editor for the background wake setting', () => {
  const file = resolve(process.cwd(), 'src/features/thread/Header.tsx');
  expect(staticSpecifiers(file, readFileSync(file, 'utf8'))).not.toContain('@/features/settings/Editor');
});

it.each(['src/shell/Shell.tsx', 'src/features/thread/Thread.tsx'])('shows %s before loading a file editor', { timeout: 120_000 }, async (entry) => {
  const graph = await entryGraph(entry);
  expect(graph.modules.has('ui/CodeView.tsx'), wayTo(graph, 'ui/CodeView.tsx').join(' -> ')).toBe(false);
  const editors = [...graph.packages].filter(([name]) => name.startsWith('@codemirror/') || name.startsWith('@lezer/'));
  expect(editors).toEqual([]);
});

it('shows a thread and its limits without loading the form for changing a cap', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/features/thread/Thread.tsx');
  const form = 'features/usage/CapDialog.tsx';
  expect(graph.modules.has(form), wayTo(graph, form).join(' -> ')).toBe(false);
});

it('opens a conversation before loading the dialogs for moving, binding or scheduling it', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/features/thread/Thread.tsx');
  for (const dialog of ['features/thread-move/MoveToTopic.tsx', 'features/start/SetUpThread.tsx', 'features/schedules/Editor.tsx']) {
    expect(graph.modules.has(dialog), wayTo(graph, dialog).join(' -> ')).toBe(false);
  }
});

it('opens an ordinary conversation without downloading the settings catalogue for model help and program forks', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/features/thread/Thread.tsx');
  for (const later of ['generated/settings-registry.ts', 'features/help/SettingHelpGroup.tsx', 'features/ask/Keep.tsx']) {
    expect(graph.modules.has(later), wayTo(graph, later).join(' -> ')).toBe(false);
  }
});

it('opens a conversation without loading the access editor for a future fork', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/features/thread/ImmediateFork.tsx');
  for (const later of ['features/start/AgentAccess.tsx', 'features/access/AgentAccessChoice.tsx', 'features/settings/ValueControl.tsx']) {
    expect(graph.modules.has(later), wayTo(graph, later).join(' -> ')).toBe(false);
  }
});
