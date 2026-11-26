import { TriangleAlert } from 'lucide-react';
import { Component, type ErrorInfo, type ReactNode } from 'react';

interface State {
  error: Error | null;
}

/**
 * Catches an error thrown while drawing, and says what it was. Without it React
 * unmounts the whole tree and the window goes blank with no explanation.
 */
export class ErrorBoundary extends Component<{ children: ReactNode; resetKey?: string }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('vKore: the page failed to draw', error, info.componentStack);
  }

  componentDidUpdate(previous: { resetKey?: string }) {
    // Moving to another page gives it a fresh chance.
    if (this.state.error && previous.resetKey !== this.props.resetKey) this.setState({ error: null });
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="card crash" role="alert">
        <header className="card-head">
          <h2><TriangleAlert size={20} /> This page could not be drawn</h2>
        </header>
        <p className="muted">The rest of vKore still works. What went wrong:</p>
        <pre className="text-preview">{`${error.name}: ${error.message}\n${(error.stack ?? '').split('\n').slice(1, 6).join('\n')}`}</pre>
        <button onClick={() => this.setState({ error: null })}>Try again</button>
      </div>
    );
  }
}
