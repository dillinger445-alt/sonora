/**
 * Catches an error while showing a view, so one broken screen does not
 * blank the whole window: the rest of the window keeps working, and the
 * screen offers to try again.
 */

import { Component, type ErrorInfo, type ReactNode } from 'react';
import { LuTriangleAlert } from 'react-icons/lu';

interface Props {
  children: ReactNode;
  /** What failed, for the message ("This screen", "SONORA"). */
  what?: string;
  /**
   * A change of this value clears the error (going to another screen). The
   * children are not remounted when it changes and nothing failed, so focus
   * and state survive a change of route within a screen.
   */
  resetKey?: string;
  /** Shown instead of the default retry, for the window's last resort. */
  fallback?: (error: Error, retry: () => void) => ReactNode;
}

interface State {
  error: Error | null;
}

export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error('SONORA view error', error, info.componentStack);
  }

  override componentDidUpdate(prev: Props): void {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  retry = (): void => this.setState({ error: null });

  override render(): ReactNode {
    const { error } = this.state;
    if (!error) return this.props.children;
    if (this.props.fallback) return this.props.fallback(error, this.retry);
    return (
      <div className="view-error" role="alert">
        <LuTriangleAlert className="view-error-icon" aria-hidden />
        <h2 className="view-error-title">{this.props.what ?? 'This screen'} ran into a problem</h2>
        <p className="view-error-body selectable">{error.message || 'Something unexpected happened.'}</p>
        <p className="faint">Your library and files are not affected. Other screens keep working.</p>
        <div className="view-error-actions">
          <button type="button" className="btn btn-primary" onClick={this.retry}>
            <span className="btn-label">Try again</span>
          </button>
          <button type="button" className="btn btn-secondary" onClick={() => window.location.reload()}>
            <span className="btn-label">Reload the window</span>
          </button>
        </div>
      </div>
    );
  }
}
