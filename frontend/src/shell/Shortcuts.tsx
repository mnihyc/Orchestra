/** Global keyboard command wiring: the finder, the bars, the areas, tabs, save, help and the context menu of what is in focus.
 *
 * The keys that work anywhere in the workbench, and the list of them that "?" opens. The root shell's
 * one listener hears the page before this screen-dependent set mounts; it never takes a key from an input method that is composing, and never takes a
 * key that types from a field. Every key is bound in the shell's one book (`keys.ts`), and the list
 * is drawn from that book, so it can say nothing that is not bound.
 *
 * So far: the finder and the search, the three bars, going from area to area, a tab of the group in
 * focus by its number, closing the tab in front, the help of the screen, the places a person goes to
 * by two keys, and the list itself. Saving a file arrives with the code view.
 */
import { useNavigate } from '@tanstack/react-router';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { orgsKey } from '@/api/nodes';
import { queryClient } from '@/api/query';
import { KeyList } from '@/features/help/KeyList';
import { useHelpOpener } from '@/features/help/open';
import { useHelpState } from '@/features/help/state';
import { findGroup, findTab } from '@/features/workbench/layout';
import { useShape } from '@/features/workbench/Narrow';
import { useWorkbench } from '@/features/workbench/store';
import { isClosable } from '@/features/workbench/Tabs';
import type { Org } from '@/generated/api-types';
import { bindAction, onKeyListRequested } from '@/shell/keys';
import { useSelectedOrg } from '@/shell/org';
import { Dialog } from '@/ui/Dialog';
import { focusArea } from '@/shell/areas';

/** The group the keyboard is in, or the one it was last in. */
function groupInFocus(): string | null {
  const within = document.activeElement?.closest<HTMLElement>('[data-view-group]');
  const panel = within?.querySelector<HTMLElement>('[role="tabpanel"]');
  const fromPanel = panel?.id.replace(/^panel-/, '');
  return fromPanel ?? useWorkbench.getState().focusedGroup;
}

export interface ShortcutsProps {
  listOpen: boolean;
  onListOpenChange: (open: boolean) => void;
}

export function Shortcuts({ listOpen, onListOpenChange }: ShortcutsProps) {
  const navigate = useNavigate();
  const openHelp = useHelpOpener();
  const shape = useShape();
  const beside = shape === 'wide';

  useEffect(() => onKeyListRequested(() => onListOpenChange(true)), [onListOpenChange]);

  useEffect(() => {
    const workbench = useWorkbench.getState;
    const bindWork = (id: Parameters<typeof bindAction>[0], run: Parameters<typeof bindAction>[1]) => bindAction(id, (event) => {
      if (!useHelpState.getState().overlay) return run(event);
      useHelpState.setState({ overlay: false });
      requestAnimationFrame(() => { run(event); });
      return true;
    });
    // The order of binding is the order of the list.
    const undo = [
      bindWork('search', () => {
        // On the search page the keys give its field the caret, keeping what is typed there.
        const state = workbench();
        const tab = state.activeContent === null ? null : findTab(state.layout, state.activeContent)?.tab;
        const root = tab === null || tab === undefined ? null : document.querySelector<HTMLElement>(`[data-view-instance="${CSS.escape(tab.id)}"]`);
        const field = root?.querySelector<HTMLInputElement>('form[role="search"] input[data-autofocus]');
        if (tab?.of?.href?.split(/[?#]/)[0] === '/search' && field !== null && field !== undefined) {
          field.focus();
          field.select();
          return;
        }
        void navigate({ to: '/search' });
      }),
      bindWork('left-bar', () => workbench().revealBar('left', beside)),
      bindWork('right-bar', () => workbench().revealBar('right', beside)),
      bindWork('bottom', () => workbench().toggle('bottom', beside)),
      bindAction('next-area', () => focusArea(1)),
      bindAction('previous-area', () => focusArea(-1)),
      ...TAB_ACTIONS.map((id, index) =>
        bindWork(
          id,
          () => {
            const group = findGroup(workbench().layout, groupInFocus() ?? '')?.group;
            const tab = group?.tabs[index];
            if (tab === undefined) return false;
            workbench().focus(tab.id);
            requestAnimationFrame(() => document.getElementById(`tab-${tab.id}`)?.focus());
            return true;
          },
        ),
      ),
      bindAction('focus-mode', () => { const current = workbench(); current.setFocusMode(!current.focusMode); return true; }),
      bindAction(
        'close-tab',
        () => {
          if (useHelpState.getState().overlay) { useHelpState.setState({ overlay: false }); return true; }
          const group = findGroup(workbench().layout, groupInFocus() ?? '')?.group;
          const tabId = group?.active ?? null;
          if (tabId === null || !isClosable(workbench().layout, tabId)) return false;
          workbench().close({ kind: 'tab', tab: tabId });
          return true;
        },
      ),
      bindAction(
        'help',
        () => { openHelp(); return true; },
      ),
      bindAction('inbox', () => void navigate({ to: '/inbox' })),
      bindAction('approvals', () => void navigate({ to: '/approvals' })),
      bindAction(
        'messages',
        () => {
          // Messages are an org's: the one in view.
          const slug = orgSlug();
          if (slug === null) return false;
          void navigate({ to: '/org/$slug/messages', params: { slug } });
          return true;
        },
      ),
      bindAction('list', () => onListOpenChange(true)),
    ];
    return () => {
      for (const remove of undo) remove();
    };
  }, [beside, navigate, onListOpenChange, openHelp]);

  return <ShortcutDialog open={listOpen} onOpenChange={onListOpenChange} />;
}

/** The same searchable shortcut editor opens from the account menu and its keyboard command. */
export function ShortcutDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="xl" title={t('shell.shortcuts.title')}>
      <KeyList />
    </Dialog>
  );
}

/** The nine actions that bring a tab of the group in focus to the front, by its number. */
const TAB_ACTIONS = ['tab-1', 'tab-2', 'tab-3', 'tab-4', 'tab-5', 'tab-6', 'tab-7', 'tab-8', 'tab-9'] as const;

/** The short name of the root org in view, among the orgs the person belongs to. */
function orgSlug(): string | null {
  const orgId = useSelectedOrg.getState().orgId;
  return queryClient.getQueryData<Org[]>(orgsKey)?.find((org) => org.id === orgId)?.slug ?? null;
}
