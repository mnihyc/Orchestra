/** The code the page loads first holds the shell and the screens' addresses, and none of what loads when it is first used. */
import { expect, it } from 'vitest';

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

it('holds none of the editor, the difference view, the charts, the PDF reader, the terminal or the settings registry', { timeout: 120_000 }, async () => {
  const graph = await entryGraph('src/main.tsx');
  const reached = MODULES.filter((module) => graph.modules.has(module)).map((module) => wayTo(graph, module).join(' -> '));
  expect(reached, 'modules that load on first use are reached by static imports').toEqual([]);
  const libraries = [...graph.packages].filter(([name]) => PACKAGES.some((pattern) => pattern.test(name))).map(([name, from]) => `${name} (from ${from})`);
  expect(libraries, 'libraries that load on first use are reached by static imports').toEqual([]);
});
