declare module 'virtual:orch-first-shell-words' {
  export const words: Readonly<Record<'en' | 'zh-CN', { failed: string; retry: string }>>;
}
