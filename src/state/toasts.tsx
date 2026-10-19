/** Transient notifications ("Removed 3 tracks from the library"). */

import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuCircleAlert, LuCircleCheck, LuInfo, LuTriangleAlert, LuX } from 'react-icons/lu';

export type ToastTone = 'info' | 'success' | 'warning' | 'error';

export interface ToastInput {
  tone?: ToastTone;
  title: string;
  body?: string;
  action?: { label: string; run: () => void };
  /** Milliseconds; errors stay until dismissed when 0. */
  timeout?: number;
}

interface Toast extends ToastInput {
  id: number;
  tone: ToastTone;
}

const ToastContext = createContext<((t: ToastInput) => void) | null>(null);

export function useToast(): (t: ToastInput) => void {
  const v = useContext(ToastContext);
  if (!v) throw new Error('useToast outside ToastProvider');
  return v;
}

const ICONS = { info: LuInfo, success: LuCircleCheck, warning: LuTriangleAlert, error: LuCircleAlert };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => setToasts((ts) => ts.filter((t) => t.id !== id)), []);

  const push = useCallback(
    (input: ToastInput) => {
      const id = nextId.current++;
      const tone = input.tone ?? 'info';
      setToasts((ts) => [...ts.slice(-3), { ...input, id, tone }]);
      const timeout = input.timeout ?? (tone === 'error' ? 9000 : 5000);
      if (timeout > 0) setTimeout(() => dismiss(id), timeout);
    },
    [dismiss],
  );

  const value = useMemo(() => push, [push]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toasts" role="region" aria-label="Notifications">
        <div aria-live="polite" aria-atomic="false">
          {toasts.map((t) => {
            const Icon = ICONS[t.tone];
            return (
              <div key={t.id} className={`toast toast-${t.tone}`} role={t.tone === 'error' ? 'alert' : 'status'}>
                <Icon className="toast-icon" aria-hidden />
                <div className="toast-text">
                  <div className="toast-title">{t.title}</div>
                  {t.body ? <div className="toast-body selectable">{t.body}</div> : null}
                </div>
                {t.action ? (
                  <button
                    type="button"
                    className="btn btn-ghost btn-sm"
                    onClick={() => {
                      t.action?.run();
                      dismiss(t.id);
                    }}
                  >
                    {t.action.label}
                  </button>
                ) : null}
                <button type="button" className="icon-btn icon-btn-sm" aria-label="Dismiss notification" onClick={() => dismiss(t.id)}>
                  <LuX aria-hidden />
                </button>
              </div>
            );
          })}
        </div>
      </div>
    </ToastContext.Provider>
  );
}
