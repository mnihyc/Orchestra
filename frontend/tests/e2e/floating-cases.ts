/** The complete floating-family matrix, shared without importing browser components into the runner. */
export const FLOATING_KINDS = ['popover', 'help', 'detail', 'person', 'agent', 'reference', 'tooltip', 'menu', 'submenu', 'context', 'select', 'combobox', 'paged-combobox', 'date', 'avatars', 'holder', 'suggestions', 'reader', 'fixed', 'overflow'] as const;
export type FloatingKind = (typeof FLOATING_KINDS)[number];
