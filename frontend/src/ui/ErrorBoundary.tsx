/** Keeps a fault of one part of the page to that part: what failed is replaced by what the caller draws for it, and everything round it goes on.
 */
import { Component, type ErrorInfo, type ReactNode } from 'react';

export interface ErrorBoundaryProps {
  /** What is drawn in place of the part that failed. `retry` draws the part anew. */
  fallback: (error: unknown, retry: () => void) => ReactNode;
  /** When this changes, a part that failed is drawn anew: the thing it shows is another one now. */
  resetKey?: unknown;
  children: ReactNode;
}

interface State {
  failed: boolean;
  error: unknown;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, State> {
  override state: State = { failed: false, error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { failed: true, error };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    // The one place a fault of the page is written down, for whoever looks into it.
    console.error(error, info.componentStack);
  }

  override componentDidUpdate(previous: ErrorBoundaryProps): void {
    if (this.state.failed && previous.resetKey !== this.props.resetKey) this.retry();
  }

  retry = (): void => {
    this.setState({ failed: false, error: null });
  };

  override render(): ReactNode {
    return this.state.failed ? this.props.fallback(this.state.error, this.retry) : this.props.children;
  }
}
