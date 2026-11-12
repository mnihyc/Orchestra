/** The complete floating-family matrix, shared without importing browser components into the runner. */
export const FLOATING_KINDS = ['popover', 'help', 'detail', 'person', 'agent', 'reference', 'tooltip', 'menu', 'submenu', 'context', 'select', 'combobox', 'paged-combobox', 'date', 'avatars', 'holder', 'suggestions', 'reader', 'fixed', 'overflow'] as const;
export type FloatingKind = (typeof FLOATING_KINDS)[number];

/** Layers whose content arrives after they have opened (what a list read from the server does): the harness draws each with a list that grows 300 ms later. */
export const LATE_KINDS = ['late-popover', 'late-detail', 'late-detail-hover', 'late-intent', 'late-reference', 'late-reference-hover', 'late-holder', 'late-menu', 'late-submenu', 'late-context', 'late-select'] as const;
export type LateKind = (typeof LATE_KINDS)[number];
