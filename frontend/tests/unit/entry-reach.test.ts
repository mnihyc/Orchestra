/** The code the page loads first holds the shell and the screens' addresses, and none of what loads when it is first used. */
import { expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import ts from 'typescript';

import { entryGraph, wayTo } from './entry-graph';

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

it('holds none of the editor, the difference view, the charts, the PDF reader, the terminal or the settings registry', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/main.tsx');
  const reached = MODULES.filter((module) => graph.modules.has(module)).map((module) => wayTo(graph, module).join(' -> '));
  expect(reached, 'modules that load on first use are reached by static imports').toEqual([]);
  const libraries = [...graph.packages].filter(([name]) => PACKAGES.some((pattern) => pattern.test(name))).map(([name, from]) => `${name} (from ${from})`);
  expect(libraries, 'libraries that load on first use are reached by static imports').toEqual([]);
});
