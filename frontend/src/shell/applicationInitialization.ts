/** Every entry initializer runs in its original order, once per document, after its module arrives. */
export function applicationInitialization(load: (href: string) => Promise<unknown>) {
  const started = new Map<string, Promise<unknown>>();
  return (href: string, initializers: readonly string[]): Promise<unknown> => {
    const held = started.get(href);
    if (held !== undefined) return held;
    const reading = load(href);
    const ready = reading.then((module: unknown) => {
      if (typeof module !== 'object' || module === null) throw new Error('The application module is unavailable.');
      for (const name of initializers) {
        const value: unknown = (module as Record<string, unknown>)[name];
        if (typeof value !== 'function') throw new Error('The application initializer is unavailable.');
        const initialize = value as () => unknown;
        initialize();
      }
      return module;
    });
    started.set(href, ready);
    // A transfer can be repeated. An initialization that threw retains normal module failure semantics.
    void reading.catch(() => { if (started.get(href) === ready) started.delete(href); });
    return ready;
  };
}
