declare module 'virtual:orch-vocabulary' {
  export const moduleParts: Record<string, string>;
  export const loaders: Record<string, () => Promise<Record<string, unknown>>>;
}
