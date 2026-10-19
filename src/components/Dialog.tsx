/**
 * Modal dialogs on the native <dialog> element: focus is trapped, Escape
 * closes, and the rest of the window is inert while open.
 */

import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { LuX } from 'react-icons/lu';
import { Button } from './Button.tsx';

export interface DialogProps {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
  /** Element to focus first (defaults to the first focusable control). */
  initialFocus?: string;
}

export function Dialog({ open, title, onClose, children, footer, width = 460, initialFocus }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      d.showModal();
      const target = initialFocus ? d.querySelector<HTMLElement>(initialFocus) : null;
      (target ?? d.querySelector<HTMLElement>('[data-autofocus]'))?.focus();
    } else if (!open && d.open) {
      d.close();
    }
  }, [open, initialFocus]);

  return (
    <dialog
      ref={ref}
      className="dialog"
      aria-labelledby={titleId}
      style={{ width }}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      onMouseDown={(e) => {
        // A click on the backdrop closes the dialog.
        if (e.target === ref.current) onClose();
      }}
    >
      {open ? (
        <div className="dialog-inner">
          <header className="dialog-head">
            <h2 id={titleId} className="dialog-title">
              {title}
            </h2>
            <button type="button" className="icon-btn icon-btn-sm" aria-label="Close" onClick={onClose}>
              <LuX aria-hidden />
            </button>
          </header>
          <div className="dialog-body">{children}</div>
          {footer ? <footer className="dialog-foot">{footer}</footer> : null}
        </div>
      ) : null}
    </dialog>
  );
}

export interface ConfirmOptions {
  title: string;
  body: ReactNode;
  confirmLabel: string;
  tone?: 'danger' | 'primary';
  /** Optional extra choice shown as a checkbox. */
  option?: { label: string; defaultValue?: boolean };
}

/** Promise-based confirmation: `const ok = await confirm({...})`. */
export function useConfirm(): [(o: ConfirmOptions) => Promise<{ confirmed: boolean; option: boolean }>, ReactNode] {
  const [state, setState] = useState<{ opts: ConfirmOptions; resolve: (r: { confirmed: boolean; option: boolean }) => void } | null>(null);
  const [option, setOption] = useState(false);

  const ask = (opts: ConfirmOptions) =>
    new Promise<{ confirmed: boolean; option: boolean }>((resolve) => {
      setOption(opts.option?.defaultValue ?? false);
      setState({ opts, resolve });
    });

  const close = (confirmed: boolean) => {
    if (!state) return;
    state.resolve({ confirmed, option });
    setState(null);
  };

  const node = (
    <Dialog
      open={state != null}
      title={state?.opts.title ?? ''}
      onClose={() => close(false)}
      footer={
        <>
          {/* Destructive confirmations start on Cancel, so Enter never deletes by accident. */}
          <Button variant="ghost" onClick={() => close(false)} data-autofocus={state?.opts.tone !== 'primary' ? true : undefined}>
            Cancel
          </Button>
          <Button
            variant={state?.opts.tone === 'primary' ? 'primary' : 'danger'}
            onClick={() => close(true)}
            data-autofocus={state?.opts.tone === 'primary' ? true : undefined}
          >
            {state?.opts.confirmLabel}
          </Button>
        </>
      }
    >
      <div className="confirm-body">{state?.opts.body}</div>
      {state?.opts.option ? (
        <label className="check-row">
          <input type="checkbox" className="check" checked={option} onChange={(e) => setOption(e.target.checked)} />
          <span>{state.opts.option.label}</span>
        </label>
      ) : null}
    </Dialog>
  );

  return [ask, node];
}
