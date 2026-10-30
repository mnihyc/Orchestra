// @vitest-environment node
/// <reference types="node" />
/** A label is a few words: what a button or a menu entry says on itself is said in five words at most, a name that is
 * filled in counting as one; a sentence belongs in its hint, where there is room to read one. A check over the
 * catalogue as the screens use it: the text of every `Button`, and the label of every menu entry (an entry is an
 * object that has `onSelect`). What only a screen reader hears, or a tooltip shows, is not a label here. */
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

import english from '@/i18n/en.json';

const sourceRoot = resolve(import.meta.dirname, '../../src');
/** Walking all source once takes seconds on the shared builder; this bounds the repository audit. */
const REPOSITORY_SCAN_BUDGET_MS = 20_000;
/** The most words a label has: "Open in a new tab", the common name of a common act, is five; a sixth makes a sentence. */
const MOST_WORDS = 5;

const files = (directory: string): string[] =>
  readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? (entry.name === 'generated' || entry.name === 'dev' ? [] : files(resolve(directory, entry.name))) : entry.name.endsWith('.tsx') ? [resolve(directory, entry.name)] : [],
  );

function englishOf(key: string): string | undefined {
  let value: unknown = english;
  for (const part of key.split('.')) value = typeof value === 'object' && value !== null ? (value as Record<string, unknown>)[part] : undefined;
  return typeof value === 'string' ? value : undefined;
}

/** How many words a label says: a name filled in is one, and a mark between words is none. */
function wordsOf(label: string): number {
  return label.replace(/\{\{[^}]+\}\}/g, 'X').replace(/[…“”"().,:;!?·]/g, ' ').split(/\s+/).filter((word) => word !== '').length;
}

/** Whether a translation is a label: the text a `Button` holds, or the `label` of a menu entry. */
function isLabel(call: ts.CallExpression, source: ts.SourceFile): boolean {
  for (let parent: ts.Node = call.parent; !ts.isSourceFile(parent); parent = parent.parent) {
    // Said in an attribute of whatever element: a name for a screen reader, a tooltip, an icon. Not its text.
    if (ts.isJsxAttribute(parent)) return false;
    if (ts.isPropertyAssignment(parent)) {
      if (parent.name.getText(source) !== 'label' || !ts.isObjectLiteralExpression(parent.parent)) return false;
      return parent.parent.properties.some((property) => property.name?.getText(source) === 'onSelect');
    }
    if (ts.isJsxElement(parent)) return parent.openingElement.tagName.getText(source) === 'Button';
    if (ts.isFunctionLike(parent)) return false;
  }
  return false;
}

describe('a label is a few words', () => {
  it('counts a filled-in name as one word and marks as none', () => {
    expect(wordsOf('Open in a new tab')).toBe(5);
    expect(wordsOf('Move {{name}} to {{place}}…')).toBe(4);
    expect(wordsOf('Take back the invitation to make an org')).toBe(8);
  });

  it('says what every button and menu entry does in five words at most', () => {
    const faults: string[] = [];
    for (const file of files(sourceRoot)) {
      const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 't' && node.arguments[0] !== undefined && ts.isStringLiteral(node.arguments[0]) && isLabel(node, source)) {
          const key = node.arguments[0].text;
          const label = englishOf(key);
          if (label !== undefined && wordsOf(label) > MOST_WORDS) faults.push(`${file.slice(sourceRoot.length)}: ${key} = "${label}"`);
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
    expect(faults, faults.join('\n')).toEqual([]);
  }, REPOSITORY_SCAN_BUDGET_MS);
});
