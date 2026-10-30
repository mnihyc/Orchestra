/** The frame registers the route and navigation; each feature registers its own working views. */
import { AppWindow } from 'lucide-react';
import { createContext, type ReactNode } from 'react';
import { PageView } from '@/features/workbench/PageView';
import { registerView, type ViewDefinition } from '@/features/workbench/views';
import { ACTIONS } from '@/ui/actions';

/** Kept only while older integrations migrate; content bodies no longer consume a route outlet. */
export const ScreenOutlet = createContext<ReactNode>(null);

let registered = false;

/**
 * Registers the frame's views once. `navigation` is the body of the Navigation view, given
 * from outside so that the workbench does not know the navigation.
 */
export function registerShippedViews(navigation: ViewDefinition['body'], actions?: ViewDefinition['actions'], menu?: ViewDefinition['menu']): void {
  if (registered) return;
  registered = true;
  registerView({
    name: 'screen',
    title: 'views.screen.title',
    about: 'views.screen.about',
    icon: AppWindow,
    screens: 'all',
    area: 'main',
    role: 'content',
    body: PageView,
  });
  registerView({ name: 'page', title: 'views.screen.title', about: 'views.screen.about', icon: AppWindow, screens: 'all', area: 'main', role: 'content', body: PageView });
  registerView({
    name: 'navigation',
    title: 'views.navigation.title',
    about: 'views.navigation.about',
    icon: ACTIONS.navigation.Icon,
    screens: 'all',
    area: 'left',
    body: navigation,
    actions,
    menu,
  });
}
