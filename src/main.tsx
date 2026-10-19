import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App.tsx';
import { createBridge } from './api/createBridge.ts';
import './styles/index.css';

const container = document.getElementById('root');
if (!container) throw new Error('missing #root');
const root = createRoot(container);

createBridge().then(
  (bridge) => {
    root.render(
      <StrictMode>
        <App bridge={bridge} />
      </StrictMode>,
    );
  },
  (err: unknown) => {
    // Nothing else can work without a bridge; say so plainly.
    container.textContent = `SONORA could not start its interface bridge: ${err instanceof Error ? err.message : String(err)}`;
  },
);
