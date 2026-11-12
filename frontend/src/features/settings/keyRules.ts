/** Explicit selections from the current operation catalogue; actions never leave their rule's target. */
import type { KeyAction, KeyOperation, KeyTargetKind, Operation, OperationVariant } from '@/generated/api-types';

export const KEY_ACTIONS = ['read', 'write', 'manage'] as const satisfies readonly KeyAction[];

export function hasKeySubtree(kind: KeyTargetKind): boolean {
  return ['org', 'project', 'topic', 'thread', 'document_container', 'account'].includes(kind);
}

/** A resource filter limits the offered facets; the review still shows every facet of each operation. */
export function ruleOperations(catalogue: readonly Operation[], resources: readonly KeyTargetKind[]): Operation[] {
  return catalogue.filter((operation) => operation.delegation === 'business').map((operation) => ({
    ...operation,
    variants: operation.variants.filter((variant) => variant.facets.some((facet) => facet.resources.some((kind) => resources.includes(kind)))),
  })).filter((operation) => operation.variants.length > 0);
}

export function offeredActions(variant: OperationVariant, resources: readonly KeyTargetKind[]): KeyAction[] {
  return KEY_ACTIONS.filter((action) => variant.facets.some((facet) => facet.action === action && facet.resources.some((kind) => resources.includes(kind))));
}

/** Only an explicit preset click expands today's exact variant IDs. A catalogue refresh never calls it. */
export function presetOperations(catalogue: readonly Operation[], resources: readonly KeyTargetKind[], actions: readonly KeyAction[]): KeyOperation[] {
  return ruleOperations(catalogue, resources).flatMap((operation) => operation.variants.flatMap((variant) => {
    const selected = offeredActions(variant, resources).filter((action) => actions.includes(action));
    return selected.length === 0 ? [] : [{ operation: variant.operation, actions: selected }];
  }));
}

/** Change only the explicitly named selections, preserving unavailable historical IDs for review. */
export function selectOperations(value: readonly KeyOperation[], variants: readonly OperationVariant[], resources: readonly KeyTargetKind[], actions: readonly KeyAction[], checked: boolean): KeyOperation[] {
  const next = new Map(value.map((item) => [item.operation, new Set(item.actions)]));
  for (const variant of variants) {
    const selected = next.get(variant.operation) ?? new Set<KeyAction>();
    for (const action of offeredActions(variant, resources)) if (actions.includes(action)) {
      if (checked) selected.add(action); else selected.delete(action);
    }
    if (selected.size === 0) next.delete(variant.operation); else next.set(variant.operation, selected);
  }
  return [...next].map(([operation, actions]) => ({ operation, actions: KEY_ACTIONS.filter((action) => actions.has(action)) }));
}
