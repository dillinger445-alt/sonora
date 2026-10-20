import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.tsx';
import { createBridge } from './api/createBridge.ts';
import { ErrorBoundary } from './components/ErrorBoundary.tsx';
import './styles/index.css';

const container = document.getElementById('root');
if (!container) throw new Error('missing #root');
const root = createRoot(container);

createBridge().then(
  (bridge) => {
    root.render(
      <StrictMode>
        {/* The last resort: a window that cannot show anything says so. */}
        <ErrorBoundary
          what="SONORA"
          fallback={(error) => (
            <div className="view-error" role="alert">
              <h1 className="view-error-title">SONORA ran into a problem</h1>
              <p className="view-error-body selectable">{error.message || 'Something unexpected happened.'}</p>
              <p className="faint">Your library and your files are not affected. Reloading the window starts the interface again.</p>
              <div className="view-error-actions">
                <button type="button" className="btn btn-primary" onClick={() => window.location.reload()}>
                  <span className="btn-label">Reload the window</span>
                </button>
              </div>
            </div>
          )}
        >
          <App bridge={bridge} />
        </ErrorBoundary>
      </StrictMode>,
    );
  },
  (err: unknown) => {
    // Nothing else can work without a bridge; say so plainly.
    container.textContent = `SONORA could not start its interface bridge: ${err instanceof Error ? err.message : String(err)}`;
  },
);
