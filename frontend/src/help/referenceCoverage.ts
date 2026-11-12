/** Declared settings, option fields, states and API errors must have authored user-reference rows. */
import { Lexer, type Token, type Tokens } from 'marked';
import { articleFirstLine, type Guide } from './guide';

const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Named properties are finite declarations; a map's user-selected keys remain a wildcard. */
export function schemaFieldPaths(schema: unknown, prefix = ''): string[] {
  if (!object(schema)) return [];
  const paths: string[] = [];
  if (object(schema.properties)) for (const [key, child] of Object.entries(schema.properties)) {
    const path = prefix === '' ? key : `${prefix}.${key}`;
    paths.push(path, ...schemaFieldPaths(child, path));
  }
  for (const key of ['items', 'additionalProperties']) paths.push(...schemaFieldPaths(schema[key], prefix === '' ? '*' : `${prefix}.*`));
  for (const key of ['oneOf', 'anyOf', 'allOf']) {
    const variants = schema[key];
    if (Array.isArray(variants)) for (const child of variants) paths.push(...schemaFieldPaths(child, prefix));
  }
  return [...new Set(paths)];
}

function enumValues(schema: unknown): string[] {
  if (!object(schema)) return [];
  const values: string[] = [];
  if (Array.isArray(schema.enum)) for (const value of schema.enum) if (typeof value === 'string') values.push(value);
  for (const key of ['oneOf', 'anyOf']) {
    const variants = schema[key];
    if (!Array.isArray(variants)) continue;
    for (const value of variants) {
      if (!object(value)) continue;
      if (typeof value.const === 'string') values.push(value.const);
      else if (object(value.properties)) {
        const tags = Object.values(value.properties).flatMap((field) => object(field) && typeof field.const === 'string' ? [field.const] : []);
        if (tags.length > 0) values.push(...tags);
        else if (Array.isArray(value.required) && value.required.length === 1 && typeof value.required[0] === 'string' && Object.hasOwn(value.properties, value.required[0])) values.push(value.required[0]);
      }
    }
  }
  return values;
}

/** No count, vendor list or sample fixture can make a newly declared member disappear from coverage. */
export function requiredHelpReferences(settings: readonly { key: string; schema: unknown }[], openapi: unknown, declarations: unknown): string[] {
  const required = new Set<string>();
  for (const setting of settings) {
    required.add(`setting:${setting.key}`);
    for (const path of schemaFieldPaths(setting.schema)) required.add(`option:${setting.key}.${path}`);
  }
  if (object(openapi) && object(openapi.components) && object(openapi.components.schemas)) {
    for (const [name, schema] of Object.entries(openapi.components.schemas)) {
      if (/(Options|OptionsBody)$/.test(name)) for (const path of schemaFieldPaths(schema)) required.add(`option:${name}.${path}`);
      if (/(State|Status|Phase)$/.test(name)) for (const value of enumValues(schema)) required.add(`state:${name}.${value}`);
    }
    for (const code of enumValues(openapi.components.schemas.ErrorCode)) required.add(`error:${code}`);
  }
  const walk = (value: unknown, path: string) => {
    if (Array.isArray(value)) {
      value.forEach((child, index) => {
        const name = object(child) && typeof child.id === 'string' ? child.id : object(child) && typeof child.name === 'string' ? child.name : String(index);
        walk(child, path === '' ? name : `${path}.${name}`);
      });
      return;
    }
    if (!object(value)) return;
    if (object(value.options_schema)) for (const field of schemaFieldPaths(value.options_schema)) required.add(`option:${path}.${field}`);
    for (const [name, child] of Object.entries(value)) if (name !== 'options_schema') walk(child, path === '' ? name : `${path}.${name}`);
  };
  walk(declarations, '');
  return [...required].sort();
}

const table = (token: Token): token is Tokens.Table => token.type === 'table';

/** Only a rendered table row with a real explanation counts; comments and code examples do not. */
export function documentedReferences(body: string): { id: string; explanation: string }[] {
  return Lexer.lex(body).filter(table).flatMap((token) => token.rows.flatMap((row) => {
    const key = row[0]?.text.trim() ?? '';
    const found = /^`((?:setting|option|state|error):[^`]+)`$/.exec(key);
    return found?.[1] === undefined ? [] : [{ id: found[1], explanation: row.slice(1).map((cell) => articleFirstLine({ body: cell.text })).join(' ').trim() }];
  }));
}

export function checkReferenceCoverage(guide: Guide, userArticles: readonly string[], required: readonly string[]): string[] {
  const failures: string[] = [];
  const expected = new Set(required);
  for (const language of ['en', 'zh-CN']) {
    const entries = guide.articles.filter((article) => userArticles.includes(article.id)).flatMap((article) => {
      const body = language === 'en' ? article.body : article.translations?.[language]?.body;
      return body === undefined ? [] : documentedReferences(body);
    });
    const seen = new Set<string>();
    for (const entry of entries) {
      if (seen.has(entry.id)) failures.push(`reference.duplicate: ${language}:${entry.id}`);
      seen.add(entry.id);
      if (entry.id.startsWith('setting:') && !expected.has(entry.id)) failures.push(`reference.setting_unknown: ${language}:${entry.id}`);
      if (entry.explanation === '' || entry.explanation === entry.id || (language === 'zh-CN' && !/\p{Script=Han}/u.test(entry.explanation))) failures.push(`reference.explanation_missing: ${language}:${entry.id}`);
    }
    for (const id of required) if (!seen.has(id)) failures.push(`reference.missing: ${language}:${id}`);
  }
  return [...new Set(failures)];
}
