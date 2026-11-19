/** Error positions and plugin schema navigation preserve the configuration's actual keys and values. */
import { expect, it } from 'vitest';
import { jsonRanges, problemRange } from '@/features/resources/templates/JsonSpec';
import { newValue, replaceAt, resolveSchema, schemaAt, templateExamples, valueAt } from '@/features/resources/templates/schema';
import en from '@/i18n/en.json';
import zh from '@/i18n/zh-CN.json';

it('points an error to its nested array entry and escaped property rather than a same-named key elsewhere', () => {
  const text = '{\n  "first": {"memory_mb": 1},\n  "mounts": [\n    {"target": "/one"},\n    {"target": "/two"}\n  ],\n  "a/b~c": {"memory_mb": 0}\n}';
  expect(problemRange(text, '/mounts/1/target').line).toBe(5);
  expect(problemRange(text, '/a~1b~0c/memory_mb').line).toBe(7);
  expect(problemRange(text, '/a~1b~0c/missing').line).toBe(7);
  expect(jsonRanges(text).some((range) => range.pointer === '/mounts/0/target')).toBe(true);
});

it('points to a nested value beyond the editor initial parse viewport', () => {
  // Put the target past CodeMirror's initial 3000-character viewport, without adding extra lines.
  const padding = 'x'.repeat(4096);
  const text = `{\n  "padding": "${padding}",\n  "mounts": [\n    {"target": "/one"},\n    {"target": "/two"}\n  ]\n}`;
  const target = problemRange(text, '/mounts/1/target');
  expect(target.line).toBe(5);
  expect(text.slice(target.from, target.to)).toBe('"target": "/two"');
  expect(jsonRanges(text).some((range) => range.pointer === '/mounts/1/target')).toBe(true);
});

it('reads a local reference and the chosen object alternative without treating root constraints as a form shape', () => {
  const schema = { $defs: { amount: { type: 'integer', minimum: 1 } }, type: 'object', properties: { memory: { $ref: '#/$defs/amount' }, context: { anyOf: [ { type: 'object', required: ['blob'], properties: { blob: { type: 'string' } } }, { type: 'object', required: ['git'], properties: { git: { type: 'string' } } }, { type: 'null' } ] } }, oneOf: [{ required: ['memory'] }] };
  expect(Object.keys(resolveSchema(schema, schema).properties as object)).toEqual(['memory', 'context']);
  expect(schemaAt(schema, ['memory'])).toMatchObject({ type: 'integer', minimum: 1 });
  expect(schemaAt(schema, ['context', 'git'], { context: { git: 'https://example.org/repo' } })).toMatchObject({ type: 'string' });
  expect(newValue(schema, { type: 'object', required: ['name'], properties: { name: { type: 'string' }, timeout: { type: 'number', default: 300 } } })).toEqual({ name: '' });
});

it('accepts only complete driver examples and keeps their configuration unchanged', () => {
  const spec = { image: 'team/dev:1', extension: { unchanged: true } };
  expect(templateExamples({ 'x-orch-examples': [{ id: 'one', title: 'Example', description: 'A complete configuration.', spec }, { title: 'Incomplete' }] })).toEqual([{ id: 'one', name: 'Example', description: 'A complete configuration.', spec }]);
  expect(templateExamples({'x-orch-examples':[{id:'new',name:'Node',description:'A web project.',spec},{id:'blank',name:'',description:'',spec}]})).toEqual([{id:'new',name:'Node',description:'A web project.',spec}]);
});

it('edits and resets a sparse nested field without replacing plugin siblings or keys containing slashes', () => {
  const original = { image: 'team/dev:1', resources: { cores: 8, future_limit: 9 }, 'a/b~c': { memory_mb: 512, extension: { keep: true } } };
  const edited = replaceAt(original, ['a/b~c', 'memory_mb'], 1024);
  expect(valueAt(edited, ['a/b~c', 'memory_mb'])).toBe(1024);
  expect(edited).toEqual({ ...original, 'a/b~c': { memory_mb: 1024, extension: { keep: true } } });
  expect(replaceAt(edited, ['resources', 'cores'], undefined)).toEqual({ ...edited, resources: { future_limit: 9 } });
  expect(replaceAt({ image: 'team/dev:1', resources: { cores: 8 } }, ['resources', 'cores'], undefined)).toEqual({ image: 'team/dev:1' });
  expect(original['a/b~c'].memory_mb).toBe(512);
});

it('ships matching field references and editor words in both languages', () => {
  const keys = (value: unknown, path = ''): string[] => typeof value === 'object' && value !== null ? Object.entries(value).flatMap(([key, child]) => keys(child, `${path}.${key}`)) : [path];
  expect(keys(en.templates.fields).sort()).toEqual(keys(zh.templates.fields).sort());
  expect(keys(en.templates.editor).sort()).toEqual(keys(zh.templates.editor).sort());
  expect(keys(en.templates.hostAdvice).sort()).toEqual(keys(zh.templates.hostAdvice).sort());
  expect(keys(en.templates.choices).sort()).toEqual(keys(zh.templates.choices).sort());
  expect(keys(en.templateSettings).sort()).toEqual(keys(zh.templateSettings).sort());
  for(const branch of ['purposeChoices','sourceChoices','contextChoices','driverChoices','modeChoices'] as const) {
    expect(keys(en.templates[branch]).sort()).toEqual(keys(zh.templates[branch]).sort());
    for(const catalogue of [en,zh]) for(const choice of Object.values(catalogue.templates[branch])) {
      expect(choice.name.trim()).not.toBe('');expect(choice.description.trim()).not.toBe('');
    }
  }
});
