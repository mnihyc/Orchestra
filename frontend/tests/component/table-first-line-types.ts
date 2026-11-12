/** The browser-only fixture's callable surface, shared without importing its React tree into Node. */
export type TableCase = 'mixed' | 'small' | 'resource';
export interface TableHarness {
  mountTableCase(kind: TableCase, locale: 'en' | 'zh-CN', theme: 'light' | 'dark'): Promise<void>;
}
