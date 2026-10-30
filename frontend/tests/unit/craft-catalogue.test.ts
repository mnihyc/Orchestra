// @vitest-environment node
/// <reference types="node" />
/** Repository boundaries keep state keys out of ordinary acts and names and signs in one catalogue. */
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

import { ACTIONS } from '@/ui/actions';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

const sourceRoot = resolve(import.meta.dirname, '../../src');
/** Walking all source once takes seconds on the shared builder; this bounds the repository audit. */
const REPOSITORY_SCAN_BUDGET_MS = 20_000;
const files = (directory: string): string[] => readdirSync(directory, { withFileTypes: true }).flatMap((entry) => entry.isDirectory() ? entry.name === 'generated' ? [] : files(resolve(directory, entry.name)) : entry.name.endsWith('.tsx') ? [resolve(directory, entry.name)] : []);
const stateKey = (key: string) => key.startsWith('state.') || key.startsWith('ports.state.') || key.startsWith('ports.portState.') || key.startsWith('ports.connectionState.') || ['ports.copied', 'ports.copyRefused', 'export.list.downloading', 'export.list.downloadingAt'].includes(key) || key.startsWith('documents.access.') || ['access.state.private', 'access.state.follows', 'access.state.custom', 'chat.bell.notChosen'].includes(key) || key.startsWith('chat.bell.chosen.');

describe('a button says its act, never a state', () => {
  it('keeps every ordinary pressed translation in the source away from the state catalogue', () => {
    const faults: string[] = [];
    for (const file of files(sourceRoot)) {
      const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
      const visit = (node: ts.Node) => {
        if (ts.isCallExpression(node) && node.arguments[0] !== undefined && ts.isStringLiteral(node.arguments[0]) && stateKey(node.arguments[0].text)) {
          for (let parent = node.parent; !ts.isSourceFile(parent); parent = parent.parent) {
            if (ts.isObjectLiteralExpression(parent)) {
              const fields = parent.properties.filter(ts.isPropertyAssignment);
              const ordinary = fields.some((field) => ['onSelect', 'run', 'onClick'].includes(field.name.getText(source)));
              const value = fields.some((field) => field.name.getText(source) === 'type' && ts.isStringLiteral(field.initializer) && field.initializer.text === 'check');
              const label = fields.find((field) => field.name.getText(source) === 'label');
              if (ordinary && !value && label !== undefined && node.pos >= label.pos && node.end <= label.end) {
                faults.push(`${file.slice(sourceRoot.length)}: ${node.arguments[0].text}`);
                break;
              }
            }
            if (!ts.isJsxElement(parent) && !ts.isJsxOpeningElement(parent) && !ts.isJsxSelfClosingElement(parent)) continue;
            const opening = ts.isJsxElement(parent) ? parent.openingElement : parent;
            if (['Button', 'IconButton', 'button', 'ConfirmDialog'].includes(opening.tagName.getText(source))) {
              faults.push(`${file.slice(sourceRoot.length)}: ${node.arguments[0].text}`);
              break;
            }
          }
        }
        ts.forEachChild(node, visit);
      };
      visit(source);
    }
    expect(faults).toEqual([]);
  }, REPOSITORY_SCAN_BUDGET_MS);
});

describe('the same act has one name and sign', () => {
  it('gives every typed act a usable name in both languages without state keys', () => {
    for (const definition of Object.values(ACTIONS)) {
      expect(stateKey(definition.label)).toBe(false);
      const key = definition.label.split('.');
      const read = (catalogue: unknown): string => {
        let value = catalogue;
        for (const part of key) value = (value as Record<string, unknown>)[part];
        return String(value);
      };
      expect(read(english)).not.toBe('undefined');
      expect(read(chinese)).not.toBe('undefined');
      expect(read(english).trim()).not.toBe('');
      expect(read(chinese).trim()).not.toBe('');
      expect(definition.Icon).toBeDefined();
    }
  });
});
