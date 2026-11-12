/** Syntax-only catalogue key validation; no TypeScript program or type checker is created. */
import ts from 'typescript';

function unwrapped(node: ts.Expression): ts.Expression {
  return ts.isAsExpression(node) || ts.isTypeAssertionExpression(node) || ts.isParenthesizedExpression(node) || ts.isSatisfiesExpression(node) ? unwrapped(node.expression) : node;
}
const propertyName = (name: ts.PropertyName): string | undefined => ts.isIdentifier(name) || ts.isStringLiteralLike(name) || ts.isNumericLiteral(name) ? name.text : undefined;
function scopeOf(node: ts.Node): ts.Node {
  let at = node;
  while (!ts.isBlock(at) && !ts.isSourceFile(at)) at = at.parent;
  return at;
}
function bindingNames(name: ts.BindingName): string[] {
  return ts.isIdentifier(name) ? [name.text] : name.elements.flatMap((entry) => ts.isOmittedExpression(entry) ? [] : bindingNames(entry.name));
}
export interface UsedKey { key: string; line: number; count: boolean | null; context: string | null; ordinal: boolean; objects?: boolean; contextual?: boolean; pattern?: RegExp }

/** Resolve finite authored choices only. Native/plugin text stays runtime data, not an invented enum. */
export function translationKeys(text: string, file = 'screen.tsx', includePatterns = false): UsedKey[] {
  const source = ts.createSourceFile(file, text, ts.ScriptTarget.Latest, true, file.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
  const bindings = new Map<string, ts.VariableDeclaration[]>();
  const aliases = new Set<string>();
  const fixedTranslator = (node: ts.Expression): boolean => {
    const expression = unwrapped(node);
    return ts.isCallExpression(expression) && ts.isPropertyAccessExpression(expression.expression) && expression.expression.name.text === 'getFixedT';
  };
  const collect = (node: ts.Node) => {
    if (ts.isVariableDeclaration(node)) {
      for (const name of bindingNames(node.name)) bindings.set(name, [...(bindings.get(name) ?? []), node]);
      if (ts.isIdentifier(node.name)) {
        if (node.initializer !== undefined && fixedTranslator(node.initializer)) aliases.add(node.name.text);
      } else if (ts.isObjectBindingPattern(node.name) && node.initializer !== undefined && ts.isCallExpression(node.initializer)
        && ts.isIdentifier(node.initializer.expression) && node.initializer.expression.text === 'useTranslation') {
        for (const entry of node.name.elements) if (entry.propertyName !== undefined && propertyName(entry.propertyName) === 't' && ts.isIdentifier(entry.name)) aliases.add(entry.name.text);
      }
    }
    if (ts.isParameter(node) && ts.isIdentifier(node.name) && node.type !== undefined && ts.isTypeReferenceNode(node.type) && ts.isIdentifier(node.type.typeName) && node.type.typeName.text === 'TFunction') aliases.add(node.name.text);
    ts.forEachChild(node, collect);
  };
  collect(source);

  const declarationOf = (node: ts.Identifier): ts.VariableDeclaration | undefined => {
    for (let at: ts.Node = node; ; at = at.parent) {
      const candidates = (bindings.get(node.text) ?? []).filter((entry) => scopeOf(entry) === at);
      const declaration = candidates.filter((entry) => entry.pos < node.pos).at(-1) ?? (candidates.length === 1 ? candidates[0] : undefined);
      if (declaration !== undefined) return declaration;
      // A parameter shadows a module binding, even when it has no finite type.
      if (ts.isFunctionLike(at) && at.parameters.some((entry) => bindingNames(entry.name).includes(node.text))) return undefined;
      if (ts.isSourceFile(at)) return undefined;
    }
  };
  const literalTypes = (type: ts.TypeNode | undefined): string[] => type === undefined ? []
    : ts.isLiteralTypeNode(type) && (ts.isStringLiteralLike(type.literal) || ts.isNumericLiteral(type.literal)) ? [type.literal.text]
      : ts.isUnionTypeNode(type) ? type.types.flatMap(literalTypes) : ts.isParenthesizedTypeNode(type) ? literalTypes(type.type) : [];

  const boundValues = (name: ts.BindingName, values: ts.Expression[], wanted: string): ts.Expression[] => {
    if (ts.isIdentifier(name)) return name.text === wanted ? values : [];
    return name.elements.flatMap((entry, index) => {
      if (ts.isOmittedExpression(entry) || !bindingNames(entry.name).includes(wanted)) return [];
      const picked = ts.isObjectBindingPattern(name)
        ? values.flatMap((value) => propertyValues(value, entry.propertyName === undefined && ts.isIdentifier(entry.name) ? entry.name.text : entry.propertyName === undefined ? undefined : propertyName(entry.propertyName), new Set()))
        : values.flatMap((value) => { const item = choices(value)[index]; return item === undefined ? [] : [item]; });
      return boundValues(entry.name, picked, wanted);
    });
  };
  const choices = (given: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): ts.Expression[] => {
    const node = unwrapped(given);
    if (seen.has(node)) return [];
    const next = new Set(seen).add(node);
    if (ts.isArrayLiteralExpression(node)) return node.elements.flatMap((entry) => ts.isOmittedExpression(entry) ? [] : ts.isSpreadElement(entry) ? choices(entry.expression, next) : [entry]);
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      if (declaration?.initializer !== undefined) return boundValues(declaration.name, [declaration.initializer], node.text).flatMap((value) => choices(value, next));
    }
    return [];
  };
  const objects = (given: ts.Expression, seen: ReadonlySet<ts.Node>): ts.ObjectLiteralExpression[] => {
    const node = unwrapped(given);
    if (seen.has(node)) return [];
    if (ts.isObjectLiteralExpression(node)) return [node];
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      if (declaration?.initializer !== undefined) return boundValues(declaration.name, [declaration.initializer], node.text).flatMap((value) => objects(value, new Set(seen).add(node)));
      return parameterValues(node).flatMap((value) => objects(value, new Set(seen).add(node)));
    }
    const next = new Set(seen).add(node);
    if (ts.isPropertyAccessExpression(node)) return propertyValues(node.expression, node.name.text, next).flatMap((value) => objects(value, next));
    if (ts.isElementAccessExpression(node)) {
      const values = choices(node.expression, next);
      return (values.length === 0 ? propertyValues(node.expression, undefined, next) : values).flatMap((value) => objects(value, next));
    }
    return [];
  };
  const propertyValues = (object: ts.Expression, key: string | undefined, seen: ReadonlySet<ts.Node>): ts.Expression[] => objects(object, seen).flatMap((entry) => entry.properties.flatMap((property) => {
    if (ts.isSpreadAssignment(property)) return propertyValues(property.expression, key, new Set(seen).add(entry));
    if (!ts.isPropertyAssignment(property) && !ts.isShorthandPropertyAssignment(property)) return [];
    if (key !== undefined && propertyName(property.name) !== key) return [];
    return [ts.isShorthandPropertyAssignment(property) ? property.name : property.initializer];
  }));
  const parameterValues = (node: ts.Identifier): ts.Expression[] => {
    for (let at: ts.Node = node; ; at = at.parent) {
      if (ts.isFunctionLike(at)) {
        const index = at.parameters.findIndex((entry) => bindingNames(entry.name).includes(node.text));
        if (index >= 0) {
          const call = at.parent;
          if (index === 0 && ts.isCallExpression(call) && ts.isPropertyAccessExpression(call.expression) && ['map', 'flatMap', 'forEach', 'filter', 'find', 'some', 'every'].includes(call.expression.name.text)) {
            const parameter = at.parameters[index];
            return parameter === undefined ? [] : boundValues(parameter.name, choices(call.expression.expression), node.text);
          }
          return [];
        }
      }
      if (ts.isForOfStatement(at) && ts.isVariableDeclarationList(at.initializer)) {
        const entry = at.initializer.declarations.find((declaration) => bindingNames(declaration.name).includes(node.text));
        if (entry !== undefined) return boundValues(entry.name, choices(at.expression), node.text);
      }
      if (ts.isSourceFile(at)) return [];
    }
  };
  const append = (left: readonly string[], right: readonly string[]): string[] => [...new Set(left.flatMap((one) => right.map((two) => one + two)))];
  const keys = (given: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): string[] => {
    const node = unwrapped(given);
    if (seen.has(node)) return [];
    const next = new Set(seen).add(node);
    if (ts.isStringLiteralLike(node) || ts.isNumericLiteral(node)) return [node.text];
    if (ts.isConditionalExpression(node)) return [...keys(node.whenTrue, next), ...keys(node.whenFalse, next)];
    if (ts.isArrayLiteralExpression(node)) return choices(node).flatMap((entry) => keys(entry, next));
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) return [...forms(node.left, next), ...forms(node.right, next)].every((form) => form.finite) ? append(keys(node.left, next), keys(node.right, next)) : [];
    if (ts.isTemplateExpression(node)) return node.templateSpans.every((span) => forms(span.expression, next).every((form) => form.finite)) ? node.templateSpans.reduce((parts, span) => append(append(parts, keys(span.expression, next)), [span.literal.text]), [node.head.text]) : [];
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      if (declaration?.initializer !== undefined) return boundValues(declaration.name, [declaration.initializer], node.text).flatMap((value) => keys(value, next));
      const values = parameterValues(node);
      if (values.length > 0) return values.flatMap((entry) => keys(entry, next));
      for (let at: ts.Node = node; ; at = at.parent) {
        if (ts.isFunctionLike(at)) {
          const parameter = at.parameters.find((entry) => ts.isIdentifier(entry.name) && entry.name.text === node.text);
          if (parameter !== undefined) return literalTypes(parameter.type);
        }
        if (ts.isSourceFile(at)) return literalTypes(declaration?.type);
      }
    }
    if (ts.isPropertyAccessExpression(node)) return propertyValues(node.expression, node.name.text, next).flatMap((entry) => keys(entry, next));
    if (ts.isElementAccessExpression(node)) {
      const items = choices(node.expression, next);
      if (items.length > 0) return items.flatMap((value) => keys(value, next));
      const selected = keys(node.argumentExpression, next);
      return (selected.length === 0 ? propertyValues(node.expression, undefined, next) : selected.flatMap((key) => propertyValues(node.expression, key, next))).flatMap((entry) => keys(entry, next));
    }
    return [];
  };
  const escape = (value: string): string => value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
  interface Form { regex: string; label: string; finite: boolean; fixed: string }
  const fixed = (value: string): Form => ({ regex: escape(value), label: value, finite: true, fixed: value });
  const opaque: Form = { regex: '.+', label: '${…}', finite: false, fixed: '' };
  const combine = (left: Form[], right: Form[]): Form[] => left.flatMap((one) => right.map((two) => ({
    regex: one.regex + two.regex, label: one.label + two.label, finite: one.finite && two.finite, fixed: one.fixed + two.fixed,
  })));
  const forms = (given: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): Form[] => {
    const node = unwrapped(given);
    if (seen.has(node)) return [opaque];
    const next = new Set(seen).add(node);
    if (ts.isConditionalExpression(node)) return [...forms(node.whenTrue, next), ...forms(node.whenFalse, next)];
    if (ts.isArrayLiteralExpression(node)) return choices(node).flatMap((entry) => forms(entry, next));
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      if (declaration?.initializer !== undefined) return boundValues(declaration.name, [declaration.initializer], node.text).flatMap((value) => forms(value, next));
    }
    if (ts.isTemplateExpression(node)) return node.templateSpans.reduce((parts, span) => combine(combine(parts, forms(span.expression, next)), [fixed(span.literal.text)]), [fixed(node.head.text)]);
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) return combine(forms(node.left, next), forms(node.right, next));
    const finite = keys(node);
    return finite.length > 0 ? finite.map(fixed) : [opaque];
  };
  // A partly native template checks its fixed syntax. A multi-value hole is still a hole,
  // not several purported fixed suffixes. Fully finite keys are separately enumerated above.
  const shapes = (given: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): Form[] => {
    const node = unwrapped(given);
    if (seen.has(node)) return [opaque];
    const next = new Set(seen).add(node);
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      if (declaration?.initializer !== undefined) return boundValues(declaration.name, [declaration.initializer], node.text).flatMap((value) => shapes(value, next));
    }
    if (ts.isConditionalExpression(node)) return [...shapes(node.whenTrue, next), ...shapes(node.whenFalse, next)];
    if (ts.isTemplateExpression(node)) return node.templateSpans.reduce((parts, span) => {
      const values = forms(span.expression, next);
      const value = values.length === 1 && values[0]?.finite === true ? values : [opaque];
      return combine(combine(parts, value), [fixed(span.literal.text)]);
    }, [fixed(node.head.text)]);
    if (ts.isBinaryExpression(node) && node.operatorToken.kind === ts.SyntaxKind.PlusToken) return combine(shapes(node.left, next), shapes(node.right, next));
    return forms(node);
  };
  const hasTemplate = (given: ts.Expression, seen: ReadonlySet<ts.Node> = new Set()): boolean => {
    const node = unwrapped(given);
    if (seen.has(node)) return false;
    const next = new Set(seen).add(node);
    if (ts.isTemplateExpression(node)) return true;
    if (ts.isConditionalExpression(node)) return hasTemplate(node.whenTrue, next) || hasTemplate(node.whenFalse, next);
    if (ts.isIdentifier(node)) {
      const declaration = declarationOf(node);
      return declaration?.initializer !== undefined && boundValues(declaration.name, [declaration.initializer], node.text).some((value) => hasTemplate(value, next));
    }
    return false;
  };
  const translates = (node: ts.Expression): boolean => ts.isIdentifier(node) ? /^t\d*$/u.test(node.text) || aliases.has(node.text)
    : ts.isPropertyAccessExpression(node) && node.name.text === 't' || fixedTranslator(node);
  const found: UsedKey[] = [];
  const record = (node: ts.Node, first: ts.Expression, fields: readonly ts.ObjectLiteralElementLike[] = [], opaqueOptions = false) => {
    const field = (name: string): ts.Expression | undefined => {
      for (const entry of fields) {
        if (!ts.isPropertyAssignment(entry) && !ts.isShorthandPropertyAssignment(entry)) continue;
        if (propertyName(entry.name) === name) return ts.isShorthandPropertyAssignment(entry) ? entry.name : entry.initializer;
      }
      return undefined;
    };
    const context = field('context');
    const requested: { key: string; pattern?: RegExp }[] = [...new Set(keys(first))].map((key) => ({ key }));
    if (includePatterns) for (const form of shapes(first)) if (!form.finite && (form.fixed !== '' || hasTemplate(first))) requested.push({ key: form.label, pattern: new RegExp(`^${form.regex}$`, 'u') });
    for (const request of requested) found.push({ ...request, line: source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1,
      count: field('count') !== undefined ? true : opaqueOptions ? null : false,
      ordinal: field('ordinal')?.kind === ts.SyntaxKind.TrueKeyword, objects: field('returnObjects')?.kind === ts.SyntaxKind.TrueKeyword, contextual: context !== undefined,
      context: context !== undefined && ts.isStringLiteralLike(context) ? context.text : null });
  };
  const visit = (node: ts.Node) => {
    if (ts.isCallExpression(node) && translates(node.expression)) {
      const first = node.arguments[0], options = node.arguments[1];
      if (first !== undefined) record(node, first, options !== undefined && ts.isObjectLiteralExpression(options) ? options.properties : [], options !== undefined && (!ts.isObjectLiteralExpression(options) || options.properties.some(ts.isSpreadAssignment)));
    } else if (ts.isTaggedTemplateExpression(node) && translates(node.tag)) record(node, node.template);
    else if (ts.isJsxSelfClosingElement(node) || ts.isJsxOpeningElement(node)) {
      if (node.tagName.getText(source) === 'Trans') {
        const fields = node.attributes.properties.filter(ts.isJsxAttribute);
        const expression = (name: string): ts.Expression | undefined => {
          const value = fields.find((entry) => entry.name.getText(source) === name)?.initializer;
          return value === undefined ? undefined : ts.isStringLiteral(value) ? value : ts.isJsxExpression(value) ? value.expression : undefined;
        };
        const first = expression('i18nKey');
        if (first !== undefined) {
          const props = ['count', 'context', 'ordinal'].flatMap((name) => { const value = expression(name); return value === undefined ? [] : [ts.factory.createPropertyAssignment(name, value)]; });
          record(node, first, props);
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

function valueAt(catalogue: unknown, path: string): unknown {
  let value = catalogue;
  for (const key of path.split('.')) value = typeof value === 'object' && value !== null && !Array.isArray(value) ? (value as Record<string, unknown>)[key] : undefined;
  return value;
}
export function textAt(catalogue: unknown, path: string): string | null {
  const value = valueAt(catalogue, path);
  return typeof value === 'string' && value.trim() !== '' ? value : null;
}
const catalogued = new WeakMap<object, ReadonlyMap<string, readonly string[]>>();
function keyFamilies(catalogue: object): ReadonlyMap<string, readonly string[]> {
  const held = catalogued.get(catalogue);
  if (held !== undefined) return held;
  const families = new Map<string, string[]>();
  const walk = (value: unknown, path = '') => {
    if (typeof value === 'string' && value.trim() !== '') {
      const root = path.split('.')[0] ?? '';
      const family = families.get(root) ?? [];
      family.push(path, path.replace(/(?:_ordinal)?_(?:zero|one|two|few|many|other)$/u, ''));
      families.set(root, family);
    } else if (typeof value === 'object' && value !== null && !Array.isArray(value)) for (const [key, child] of Object.entries(value)) walk(child, path === '' ? key : `${path}.${key}`);
  };
  walk(catalogue); catalogued.set(catalogue, families);
  return families;
}
export function present(catalogue: unknown, used: UsedKey, language: 'en' | 'zh-CN'): boolean {
  if (used.pattern !== undefined) {
    if (typeof catalogue !== 'object' || catalogue === null) return false;
    const pattern = used.pattern;
    const families = keyFamilies(catalogue);
    const root = used.key.split('.')[0] ?? '';
    const names = root.includes('${') ? [...families.values()].flat() : families.get(root) ?? [];
    return names.some((key) => pattern.test(key));
  }
  if (used.objects === true) { const value = valueAt(catalogue, used.key); if (typeof value === 'object' && value !== null) return true; }
  if (used.contextual === true && used.context === null && typeof catalogue === 'object' && catalogue !== null) {
    const root = used.key.split('.')[0] ?? '';
    const families = keyFamilies(catalogue);
    const family = used.key.includes('.') ? families.get(root) ?? []
      : [...families].flatMap(([name, keys]) => name === root || name.startsWith(`${root}_`) ? keys : []);
    if (family.some((key) => key.startsWith(`${used.key}_`))) return true;
  }
  const keys = used.context === null ? [used.key] : [`${used.key}_${used.context}`, used.key];
  for (const key of keys) {
    if (textAt(catalogue, key) !== null) return true;
    if (used.count === false) continue;
    const plural = used.ordinal ? `${key}_ordinal` : key;
    if ((language === 'en' ? ['one', 'other'] : ['other']).every((form) => textAt(catalogue, `${plural}_${form}`) !== null)) return true;
  }
  return false;
}
