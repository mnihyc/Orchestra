// @vitest-environment node
/** The two catalogues: every text exists in both languages, with the same things to fill in, and no key stands twice in one object. */
import { describe, expect, it } from 'vitest';
import { createInstance } from 'i18next';

import english from '@/i18n/en.json';
import englishText from '@/i18n/en.json?raw';
import chinese from '@/i18n/zh-CN.json';
import chineseText from '@/i18n/zh-CN.json?raw';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';

interface Catalogue {
  [key: string]: string | Catalogue;
}

/** Every text of a catalogue by its dotted key. */
function texts(catalogue: Catalogue, prefix = ''): Map<string, string> {
  const found = new Map<string, string>();
  for (const [key, value] of Object.entries(catalogue)) {
    const path = prefix === '' ? key : `${prefix}.${key}`;
    if (typeof value === 'string') found.set(path, value);
    else for (const [inner, text] of texts(value, path)) found.set(inner, text);
  }
  return found;
}

/**
 * The keys that stand twice in one object of a JSON text, at any depth, by their dotted path. `JSON.parse` keeps the last of two equal keys and says
 * nothing, so a merge that put two sections under one name hides the first without a conflict and without an error: the text of the file is read here,
 * a string at a time, not what it parses to.
 */
export function repeatedKeys(text: string): string[] {
  interface Frame {
    object: boolean;
    keys: Set<string>;
    /** The path of the object, or of the array. */
    path: string;
    /** The key of the member being read, which is the path of what it holds. */
    key: string;
    expectKey: boolean;
  }
  const stack: Frame[] = [];
  const found: string[] = [];
  let at = 0;
  while (at < text.length) {
    const char = text.charAt(at);
    if (char === '"') {
      let end = at + 1;
      while (text.charAt(end) !== '"') end += text.charAt(end) === '\\' ? 2 : 1;
      const value = JSON.parse(text.slice(at, end + 1)) as string;
      const frame = stack.at(-1);
      if (frame?.object === true && frame.expectKey) {
        if (frame.keys.has(value)) found.push(frame.path === '' ? value : `${frame.path}.${value}`);
        frame.keys.add(value);
        frame.key = value;
        frame.expectKey = false;
      }
      at = end + 1;
      continue;
    }
    if (char === '{' || char === '[') {
      const parent = stack.at(-1);
      const path = parent === undefined ? '' : parent.object ? (parent.path === '' ? parent.key : `${parent.path}.${parent.key}`) : parent.path;
      stack.push({ object: char === '{', keys: new Set(), path, key: '', expectKey: char === '{' });
    } else if (char === '}' || char === ']') {
      stack.pop();
    } else if (char === ',') {
      const frame = stack.at(-1);
      if (frame?.object === true) frame.expectKey = true;
    }
    at += 1;
  }
  return found;
}

const placeholders = (text: string): string[] => [...text.matchAll(/\{\{\s*([\w.]+)\s*\}\}/g)].map((match) => match[1] ?? '').sort();

const en = texts(english);
const zh = texts(chinese);

type ReadTranslation = (key: string, language: 'en' | 'zh-CN') => string;

/** Compare the words rendered by the same nesting engine as the product, with no language fallback. */
async function translationReader(englishCatalogue: Catalogue, chineseCatalogue: Catalogue): Promise<ReadTranslation> {
  const instance = createInstance();
  await instance.init({
    lng: 'en', fallbackLng: false,
    resources: { en: { translation: englishCatalogue }, 'zh-CN': { translation: chineseCatalogue } },
    interpolation: { escapeValue: false },
    saveMissing: true,
    missingKeyHandler: (_languages, _namespace, key) => { throw new Error(`Missing catalogue target: ${key}`); },
  });
  // These keys come from the supplied catalogue; a missing key or nested target throws above.
  return (key, language) => instance.t(key, { lng: language, defaultValue: '' });
}

function copiedEnglishKeys(englishTexts: Map<string, string>, chineseTexts: Map<string, string>, read: ReadTranslation): string[] {
  const words = (text: string) => text.replace(/\{\{[^}]*\}\}/g, '');
  // Native tool/language/file names label choices and fields; explanatory prose still needs translation.
  const technicalNames = new Set(['Docker', 'KVM', 'MCP', 'API', 'Node', 'Python', 'Rust', 'Java', 'JavaScript', 'TypeScript', 'JSON', 'YAML', 'Markdown', 'HTML', 'PDF', 'Typst', 'HTTP', 'HTTPS', 'TCP', 'UDP', 'Git', 'SSH', 'OAuth', 'OIDC', 'SMTP', 'SSE', 'WebSocket', 'OpenAI', 'Anthropic', 'Claude', 'Claude Code', 'Codex', 'Linux', 'macOS', 'Windows', 'x86_64', 'aarch64', 'JSON Lines', 'Dockerfile', 'Ptrace', 'Tmpfs']);
  return [...englishTexts].filter(([key, source]) => {
    const chineseSource = chineseTexts.get(key);
    const text = source.includes('$t(') ? read(key, 'en') : source;
    const translated = chineseSource?.includes('$t(') === true ? read(key, 'zh-CN') : chineseSource;
    const label = key.endsWith('.name') || key.endsWith('.title') || key === 'templates.editor.json';
    const nativeName = label && technicalNames.has(text);
    const endonym = key === 'choiceCatalog.language.en.name' && text === 'English';
    return translated === text && /[a-z]{4,}/i.test(words(text)) && !nativeName && !endonym;
  }).map(([key]) => key);
}

/** Active screen writers may keep their pairs with their own choice namespace. */
function choicePaths(node: Catalogue, path = ''): string[] {
  const name = typeof node.name === 'string';
  const description = typeof node.description === 'string';
  const here = name && description || /choice/i.test(path) && (name || description) ? [path] : [];
  return [...here, ...Object.entries(node).flatMap(([key, value]) => typeof value === 'string' ? [] : choicePaths(value, path === '' ? key : `${path}.${key}`))];
}

describe('the catalogues', () => {
  it('keeps the named choice families used across enrollment, permissions and resources complete', () => {
    const required: Record<string, readonly string[]> = {
      machineRole: ['environment', 'node', 'agent', 'provider'],
      approvalMode: ['ask', 'on_request', 'auto', 'bypass'],
      sandboxLevel: ['read_only', 'workspace', 'open'],
      generalAccess: ['defaults', 'view', 'interact', 'manage', 'private', 'custom'],
      environmentKind: ['managed', 'connected'], providerKind: ['subscription', 'api'],
      templateKind: ['environment', 'provider'], mountMode: ['read_only', 'private', 'shared'],
      portAudience: ['here', 'place', 'host', 'link'],
      arrangement: ['default', 'separated', 'key_separated', 'credential_shared', 'handed_over', 'together'],
      serviceTier: ['default', 'fast', 'priority', 'flex', 'auto'],
    };
    for (const [group, values] of Object.entries(required)) for (const value of values) {
      for (const field of ['name', 'description']) {
        const key = `choiceCatalog.${group}.${value}.${field}`;
        expect(en.get(key)?.trim(), key).toBeTruthy();
        expect(zh.get(key)?.trim(), key).toBeTruthy();
      }
    }
  });

  it('keeps the navigation view labels distinct and complete in both languages', () => {
    const views = ['usage', 'performance', 'requests'];
    expect(views.map((view) => en.get(`usagePerf.viewNames.${view}`))).toEqual(['Consumption', 'Performance', 'Requests']);
    expect(views.map((view) => zh.get(`usagePerf.viewNames.${view}`))).toEqual(['用量统计', '性能', '请求']);
  });

  it('keeps every date preset named in both languages without restoring redundant descriptions', () => {
    const periods = ['7d', 'last_week', '30d', '90d', 'year', 'all'];
    expect(periods.map((period) => en.get(`usagePerf.periodNames.${period}`))).toEqual(['7 days', 'Last week', '30 days', '90 days', 'This year', 'All time']);
    expect(periods.map((period) => zh.get(`usagePerf.periodNames.${period}`))).toEqual(['7 天', '上周', '30 天', '90 天', '今年', '全部时间']);
  });

  it('covers paired choices from every screen writer, including templates, machine policy, workbench and usage', () => {
    const missing: string[] = [];
    for (const path of new Set([...choicePaths(english), ...choicePaths(chinese)])) {
      for (const field of ['name', 'description']) {
        const key = `${path}.${field}`;
        if (!en.get(key)?.trim()) missing.push(`en: ${key}`);
        if (!zh.get(key)?.trim()) missing.push(`zh-CN: ${key}`);
      }
    }
    expect(missing).toEqual([]);
  });

  it('gives every offered product choice a name and description in both languages', async () => {
    const read = await translationReader(english, chinese);
    for (const [group, choices] of Object.entries(english.choiceCatalog)) {
      for (const value of Object.keys(choices)) {
        const base = `choiceCatalog.${group}.${value}`;
        for (const field of ['name', 'description']) {
          const key = `${base}.${field}`;
          expect(en.get(key), key).toBeTruthy();
          expect(zh.get(key), key).toBeTruthy();
          if (field === 'description') expect(read(key, 'zh-CN'), key).toMatch(/\p{Script=Han}/u);
        }
        for (const language of ['en', 'zh-CN'] as const) expect(read(`${base}.name`, language), base).not.toBe(read(`${base}.description`, language));
      }
    }
  });

  it('has a real Chinese title and description for every current registry key', () => {
    for (const def of SETTINGS_REGISTRY) {
      for (const field of ['title', 'description'] as const) {
        const key = `settingRegistry.${def.key}.${field}`;
        expect(en.get(key), key).toBeTruthy();
        expect(zh.get(key), key).toMatch(/\p{Script=Han}/u);
        expect(zh.get(key), key).not.toBe(en.get(key));
      }
    }
  });
  it('have no key that stands twice in one object, at any depth: the later would hide the earlier', () => {
    expect({ 'en.json': repeatedKeys(englishText), 'zh-CN.json': repeatedKeys(chineseText) }).toEqual({ 'en.json': [], 'zh-CN.json': [] });
  });

  it('can tell a repeated key from one that only stands in two objects (what the check above stands on)', () => {
    expect(repeatedKeys('{"help": {"a": 1}, "other": {"a": 2}}')).toEqual([]);
    expect(repeatedKeys('{"help": {"a": 1}, "help": {"b": 2}}')).toEqual(['help']);
    expect(repeatedKeys('{"x": {"y": {"z": 1, "z": 2}}}')).toEqual(['x.y.z']);
    expect(repeatedKeys('{"list": [{"k": 1, "k": 2}], "text": "a, \\"b\\" {c}"}')).toEqual(['list.k']);
  });

  it('hold the same keys in both languages', () => {
    expect([...zh.keys()].sort()).toEqual([...en.keys()].sort());
  });

  it('fill in the same things in both languages', () => {
    const different = [...en].filter(([key, text]) => placeholders(text).join() !== placeholders(zh.get(key) ?? '').join());
    expect(different.map(([key]) => key)).toEqual([]);
  });

  it('leave no text empty', () => {
    const empty = [...en, ...zh].filter(([, text]) => text.trim() === '');
    expect(empty).toEqual([]);
  });

  it('are written in their own language: a Chinese text is no copy of the English one', async () => {
    const read = await translationReader(english, chinese);
    expect(copiedEnglishKeys(en, zh, read)).toEqual(['app.name']);
  });

  it('resolves identical alias syntax while still rejecting copied English behind that alias', async () => {
    const source = { alias: '$t(words)', words: 'Keep this view open.' };
    const translated = { alias: '$t(words)', words: '保持此视图打开。' };
    const read = await translationReader(source, translated);
    expect(read('alias', 'zh-CN')).toBe('保持此视图打开。');
    expect(copiedEnglishKeys(texts(source), texts(translated), read)).toEqual([]);
    const copied = await translationReader(source, source);
    expect(copiedEnglishKeys(texts(source), texts(source), copied)).toEqual(['alias', 'words']);
  });

  it('rejects a missing nested translation target', async () => {
    const read = await translationReader({ alias: '$t(words)', words: 'Keep this view open.' }, { alias: '$t(missing.target)' });
    expect(() => read('alias', 'zh-CN')).toThrow('Missing catalogue target: missing.target');
  });

  it('allows native names on labels while still rejecting an English explanation beside one', async () => {
    const source = { program: { name: 'Claude Code', description: 'Run the native harness here.' }, format: { title: 'Dockerfile' }, templates: { editor: { json: 'JSON' } } };
    const translated = { ...source, program: { ...source.program, description: '在这里运行原生框架。' } };
    const read = await translationReader(source, translated);
    expect(copiedEnglishKeys(texts(source), texts(translated), read)).toEqual([]);
    const copied = await translationReader(source, source);
    expect(copiedEnglishKeys(texts(source), texts(source), copied)).toEqual(['program.description']);
  });

  it('give English both forms of every text that counts something', () => {
    const counted = [...en.keys()].filter((key) => key.endsWith('_one') || key.endsWith('_other'));
    for (const key of counted) {
      const stem = key.replace(/_(one|other)$/, '');
      expect(en.has(`${stem}_one`), `${stem}_one`).toBe(true);
      expect(en.has(`${stem}_other`), `${stem}_other`).toBe(true);
    }
  });
});
