/** Follow class inputs and their local definitions without reading unrelated text, conditions or identifiers. */
import type { Rule } from 'eslint';
import { AST_NODE_TYPES, type TSESTree } from '@typescript-eslint/utils';

type Node = TSESTree.Node;
type Mode = 'classes' | 'values';
// These listeners run under the application's TypeScript parser. ESLint's public node type omits its extensions.
const syntax = (node: unknown): Node => node as Node;
const isClassName = (name: string) => name === 'class' || /classNames?$/i.test(name);
const isClassHelper = (name:string) => /(?:classNames?|classes)$/i.test(name);
const isFunction = (node: Node): node is TSESTree.FunctionDeclaration | TSESTree.FunctionExpression | TSESTree.ArrowFunctionExpression =>
  node.type === AST_NODE_TYPES.FunctionDeclaration || node.type === AST_NODE_TYPES.FunctionExpression || node.type === AST_NODE_TYPES.ArrowFunctionExpression;
function word(node: Node): string | null {
  if (node.type === AST_NODE_TYPES.Identifier || node.type === AST_NODE_TYPES.JSXIdentifier) return node.name;
  return node.type === AST_NODE_TYPES.Literal && typeof node.value === 'string' ? node.value : null;
}
function property(node: TSESTree.MemberExpression): string | null {
  if(node.computed&&node.property.type===AST_NODE_TYPES.Literal&&typeof node.property.value==='number')return String(node.property.value);
  return node.computed && node.property.type !== AST_NODE_TYPES.Literal ? null : word(node.property);
}
function exported(node: Node): boolean {
  const parent=node.parent;
  if(parent===undefined)return false;
  return parent.type===AST_NODE_TYPES.ExportNamedDeclaration||parent.type===AST_NODE_TYPES.ExportDefaultDeclaration||parent.type===AST_NODE_TYPES.VariableDeclaration&&parent.parent.type===AST_NODE_TYPES.ExportNamedDeclaration;
}

/** Class-producing expressions are roots; bindings and return statements supply their locally defined parts. */
export function classInputs(context: Rule.RuleContext, inspect: (node: Node, text: string) => void): Rule.RuleListener {
  const roots: { node: Node; mode: Mode }[] = [];
  const returns = new WeakMap<object, Node[]>();
  const helpers = new Set(['cx', 'clsx', 'cn', 'classNames', 'twMerge', 'twJoin']);
  const add = (node: Node | null | undefined, mode: Mode = 'values') => { if (node !== null && node !== undefined) roots.push({ node, mode }); };
  return {
    ImportDeclaration(raw: unknown) {
      const node = syntax(raw);
      if (node.type !== AST_NODE_TYPES.ImportDeclaration) return;
      if (['clsx', 'classnames', 'tailwind-merge', '@/ui/cx'].includes(node.source.value)) {
        for (const specifier of node.specifiers) helpers.add(specifier.local.name);
      }
    },
    JSXAttribute(raw: unknown) {
      const node = syntax(raw);
      if (node.type === AST_NODE_TYPES.JSXAttribute && node.name.type === AST_NODE_TYPES.JSXIdentifier && isClassName(node.name.name)) add(node.value);
    },
    Property(raw: unknown) {
      const node = syntax(raw);
      if (node.type !== AST_NODE_TYPES.Property || node.computed && node.key.type !== AST_NODE_TYPES.Literal) return;
      const name = word(node.key);
      if (name !== null && name !== 'class' && isClassName(name)) add(node.value);
    },
    VariableDeclarator(raw: unknown) {
      const node = syntax(raw);
      if (node.type === AST_NODE_TYPES.VariableDeclarator && exported(node) && node.id.type === AST_NODE_TYPES.Identifier && isClassHelper(node.id.name)) add(node.init);
    },
    AssignmentPattern(raw: unknown) {
      const node = syntax(raw);
      if (node.type === AST_NODE_TYPES.AssignmentPattern && node.left.type === AST_NODE_TYPES.Identifier && isClassName(node.left.name)) add(node.right);
    },
    FunctionDeclaration(raw: unknown) {
      const node = syntax(raw);
      if (node.type === AST_NODE_TYPES.FunctionDeclaration && exported(node) && node.id !== null && isClassHelper(node.id.name)) add(node);
    },
    AssignmentExpression(raw: unknown) {
      const node = syntax(raw);
      if (node.type !== AST_NODE_TYPES.AssignmentExpression || node.left.type !== AST_NODE_TYPES.MemberExpression) return;
      const name = property(node.left);
      if (name !== null && name !== 'class' && isClassName(name)) add(node.right);
    },
    CallExpression(raw: unknown) {
      const node = syntax(raw);
      if (node.type !== AST_NODE_TYPES.CallExpression) return;
      if (node.callee.type === AST_NODE_TYPES.Identifier && helpers.has(node.callee.name)) add(node, 'classes');
      if (node.callee.type !== AST_NODE_TYPES.MemberExpression) return;
      const name = property(node.callee);
      if (name === 'setAttribute' && node.arguments[0]?.type === AST_NODE_TYPES.Literal && node.arguments[0].value === 'class') add(node.arguments[1]);
      if (node.callee.object.type !== AST_NODE_TYPES.MemberExpression || property(node.callee.object) !== 'classList') return;
      if (name === 'add' || name === 'remove') for (const argument of node.arguments) add(argument);
      if (name === 'toggle' || name === 'replace') add(node.arguments[0]);
      if (name === 'replace') add(node.arguments[1]);
    },
    ReturnStatement(raw: unknown) {
      const node = syntax(raw);
      if (node.type !== AST_NODE_TYPES.ReturnStatement || node.argument === null) return;
      let parent: Node | null = node.parent;
      while (parent !== null) {
        if (isFunction(parent)) {
          const held = returns.get(parent) ?? [];
          held.push(node.argument); returns.set(parent, held); break;
        }
        parent = parent.type === AST_NODE_TYPES.Program ? null : parent.parent;
      }
    },
    'Program:exit'() {
      const bindings = new WeakMap<object, Node[]>();
      for (const scope of context.sourceCode.scopeManager.scopes) for (const variable of scope.variables) {
        const definitions = variable.defs.filter((definition) => definition.type === 'Variable' || definition.type === 'FunctionName').map((definition) => syntax(definition.node));
        for (const reference of variable.references) bindings.set(reference.identifier, definitions);
        for (const identifier of variable.identifiers) bindings.set(identifier, definitions);
      }
      function memberValues(object:Node,name:string|null,seen:Set<Node>):Node[] {
        if(object.type===AST_NODE_TYPES.ArrayExpression) {
          if(name===null)return object.elements.flatMap((entry)=>entry===null ? [] : resolved(entry,new Set(seen)));
          const entry=/^(?:0|[1-9]\d*)$/.test(name) ? object.elements[Number(name)] : undefined;
          return entry===null||entry===undefined ? [] : resolved(entry,new Set(seen));
        }
        if(object.type!==AST_NODE_TYPES.ObjectExpression)return [];
        if(name===null)return object.properties.flatMap((entry)=>entry.type===AST_NODE_TYPES.SpreadElement
          ? resolved(entry.argument,new Set(seen)).flatMap((spread)=>memberValues(spread,null,new Set(seen)))
          : resolved(entry.value,new Set(seen)));
        // A later own member wins over earlier spread defaults, as it does in the actual class lookup.
        for(const entry of [...object.properties].reverse()) {
          if(entry.type===AST_NODE_TYPES.SpreadElement) {
            const found=resolved(entry.argument,new Set(seen)).flatMap((spread)=>memberValues(spread,name,new Set(seen)));
            if(found.length>0)return found;
          } else if((!entry.computed||entry.key.type===AST_NODE_TYPES.Literal)&&word(entry.key)===name)return resolved(entry.value,new Set(seen));
        }
        return [];
      }
      function resolved(node: Node, seen = new Set<Node>()): Node[] {
        if (seen.has(node)) return [];
        seen.add(node);
        if (node.type === AST_NODE_TYPES.Identifier) return (bindings.get(node) ?? []).flatMap((definition) => {
          if (definition.type === AST_NODE_TYPES.VariableDeclarator) return definition.init === null ? [] : resolved(definition.init, new Set(seen));
          return isFunction(definition) ? [definition] : [];
        });
        if (node.type === AST_NODE_TYPES.TSAsExpression || node.type === AST_NODE_TYPES.TSSatisfiesExpression || node.type === AST_NODE_TYPES.TSNonNullExpression || node.type === AST_NODE_TYPES.TSTypeAssertion || node.type === AST_NODE_TYPES.ChainExpression) return resolved(node.expression, seen);
        if (node.type === AST_NODE_TYPES.MemberExpression) {
          const name = property(node);
          return resolved(node.object, seen).flatMap((object) => memberValues(object,name,new Set(seen)));
        }
        return [node];
      }
      const visited = { classes: new WeakSet<object>(), values: new WeakSet<object>() };
      function visit(expression: Node, mode: Mode) {
        if (visited[mode].has(expression)) return;
        visited[mode].add(expression);
        for (const node of resolved(expression)) {
          if (node.type === AST_NODE_TYPES.Literal) { if (typeof node.value === 'string') inspect(node, node.value); }
          else if (node.type === AST_NODE_TYPES.JSXExpressionContainer) { if (node.expression.type !== AST_NODE_TYPES.JSXEmptyExpression) visit(node.expression, mode); }
          else if (node.type === AST_NODE_TYPES.TemplateLiteral) {
            for (const part of node.quasis) inspect(part, part.value.cooked ?? part.value.raw);
            for (const part of node.expressions) visit(part, mode);
          } else if (node.type === AST_NODE_TYPES.ConditionalExpression) { visit(node.consequent, mode); visit(node.alternate, mode); }
          else if (node.type === AST_NODE_TYPES.LogicalExpression) { if (node.operator !== '&&') visit(node.left, mode); visit(node.right, mode); }
          else if (node.type === AST_NODE_TYPES.BinaryExpression && node.operator === '+') { visit(node.left, mode); visit(node.right, mode); }
          else if (node.type === AST_NODE_TYPES.ArrayExpression) { for (const part of node.elements) if (part !== null) visit(part, mode); }
          else if (node.type === AST_NODE_TYPES.SpreadElement) visit(node.argument, mode);
          else if (node.type === AST_NODE_TYPES.ObjectExpression) for (const entry of node.properties) {
            if (entry.type === AST_NODE_TYPES.SpreadElement) visit(entry.argument, mode);
            else if (mode === 'values') visit(entry.value, mode);
            else if (!entry.computed && entry.key.type === AST_NODE_TYPES.Identifier) inspect(entry.key, entry.key.name);
            else if (entry.key.type === AST_NODE_TYPES.Literal || entry.computed) visit(entry.key, mode);
          }
          else if (isFunction(node)) {
            if (node.body.type !== AST_NODE_TYPES.BlockStatement) visit(node.body, mode);
            else for (const result of returns.get(node) ?? []) visit(result, mode);
          } else if (node.type === AST_NODE_TYPES.CallExpression) {
            if (node.callee.type === AST_NODE_TYPES.Identifier && helpers.has(node.callee.name)) for (const argument of node.arguments) visit(argument, 'classes');
            else if (node.callee.type === AST_NODE_TYPES.MemberExpression && ['join', 'filter', 'concat', 'map'].includes(property(node.callee) ?? '')) {
              const method=property(node.callee);
              if(method==='map') {
                // Mapping consumes data; only the callback's outputs are class strings.
                for(const argument of node.arguments)visit(argument,mode);
                const mapper=node.arguments[0];
                if(mapper?.type===AST_NODE_TYPES.ArrowFunctionExpression&&mapper.params[0]?.type===AST_NODE_TYPES.Identifier&&mapper.body.type===AST_NODE_TYPES.Identifier&&mapper.body.name===mapper.params[0].name)visit(node.callee.object,mode);
              } else {
                visit(node.callee.object,mode);
                if(method==='concat')for(const argument of node.arguments)visit(argument,mode);
              }
            } else for (const definition of resolved(node.callee)) if (isFunction(definition)) visit(definition, mode);
          }
        }
      }
      for (const root of roots) visit(root.node, root.mode);
    },
  };
}
