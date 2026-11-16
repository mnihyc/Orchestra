/** The frame's static dependencies travel together, including widgets also opened on their own. */
import type { ChunkingContext, CodeSplittingGroup } from 'rolldown';

export function frameChunkGroup(root: string): CodeSplittingGroup {
  const closures = new WeakMap<ChunkingContext, ReadonlySet<string>>();
  return {
    debugName: 'frame',
    priority: 1,
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
      return modules.has(id) ? 'frame' : null;
    },
  };
}
