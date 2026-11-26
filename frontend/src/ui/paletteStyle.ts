/** Shared appearance of the finder and the tab search field, list and selected option. */
export const inputClasses = 'focus-inset my-2 h-(--control-md) min-w-0 flex-1 rounded-sm border border-control-border bg-surface px-3 text-md text-text placeholder:text-text-subtle';
export const listClasses = 'max-h-(--palette-list-height) overflow-y-auto overscroll-contain p-2';
export const itemClasses =
  'flex min-h-10 cursor-default items-center gap-3 rounded-md px-2 py-1 text-base text-text select-none ' +
  'data-[selected=true]:bg-accent-subtle data-[selected=true]:outline-1 data-[selected=true]:-outline-offset-1 data-[selected=true]:outline-accent data-[disabled=true]:text-text-muted';
