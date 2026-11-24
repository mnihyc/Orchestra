declare module 'virtual:orch-vocabulary' {
  export const moduleParts: Record<string, string>;
  export function readWords(locale: 'en' | 'zh-CN', part: string): Promise<Record<string, unknown>>;
}
