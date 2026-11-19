/** Each application phase carries its own static dependencies, including shared widgets. */
import type { ChunkingContext, CodeSplittingGroup } from 'rolldown';

export function staticChunkGroup(name: string, root: string, priority: number): CodeSplittingGroup {
  const closures = new WeakMap<ChunkingContext, ReadonlySet<string>>();
  return {
    debugName: name,
    priority,
    includeDependenciesRecursively: false,
    name(id, context) {
      let modules = closures.get(context);
      if (modules === undefined) {
        const reached = new Set<string>();
        const pending = [root];
        for (let next = pending.pop(); next !== undefined; next = pending.pop()) {
          if (reached.has(next)) continue;
          reached.add(next);
          pending.push(...context.getModuleInfo(next)?.importedIds ?? []);
        }
        modules = reached;
        closures.set(context, modules);
      }
      return modules.has(id) ? name : null;
    },
  };
}
