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
        <ErrorBoundary what="SONORA">
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
