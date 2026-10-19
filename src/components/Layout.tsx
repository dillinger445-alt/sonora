/** View scaffolding: header, sections, empty and error states. */

import type { ComponentType, ReactNode } from 'react';
import { LuCircleAlert } from 'react-icons/lu';
import { Button } from './Button.tsx';

type IconComponent = ComponentType<{ className?: string; 'aria-hidden'?: boolean }>;

export function ViewHeader({ title, subtitle, actions, back }: { title: ReactNode; subtitle?: ReactNode; actions?: ReactNode; back?: ReactNode }) {
  // The header doubles as the window's drag area under the macOS overlay title bar.
  return (
    <header className="view-head" data-tauri-drag-region>
      {back}
      <div className="view-titles" data-tauri-drag-region>
        <h1 className="view-title" data-tauri-drag-region>
          {title}
        </h1>
        {subtitle ? (
          <div className="view-sub" data-tauri-drag-region>
            {subtitle}
          </div>
        ) : null}
      </div>
      {actions ? <div className="view-actions">{actions}</div> : null}
    </header>
  );
}

export function Section({ title, aside, children, className }: { title?: ReactNode; aside?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={`section ${className ?? ''}`}>
      {title || aside ? (
        <div className="section-head">
          {title ? <h2 className="section-title">{title}</h2> : <span />}
          {aside ? <div className="section-aside">{aside}</div> : null}
        </div>
      ) : null}
      {children}
    </section>
  );
}

export function EmptyState({ icon: Icon, title, children, actions }: { icon?: IconComponent; title: string; children?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="empty">
      {Icon ? <Icon className="empty-icon" aria-hidden /> : null}
      <h2 className="empty-title">{title}</h2>
      {children ? <div className="empty-body">{children}</div> : null}
      {actions ? <div className="empty-actions">{actions}</div> : null}
    </div>
  );
}

export function ErrorState({ title, error, onRetry }: { title: string; error: Error | string; onRetry?: () => void }) {
  const message = typeof error === 'string' ? error : error.message;
  return (
    <div className="error-state" role="alert">
      <LuCircleAlert className="error-icon" aria-hidden />
      <div>
        <div className="error-title">{title}</div>
        <div className="error-message selectable">{message}</div>
      </div>
      {onRetry ? (
        <Button size="sm" onClick={onRetry}>
          Try again
        </Button>
      ) : null}
    </div>
  );
}

/** A label/value pair in a definition grid. */
export function Fact({ label, children, tip }: { label: string; children: ReactNode; tip?: string }) {
  return (
    <div className="fact">
      <dt className="fact-label" data-tip={tip}>
        {label}
      </dt>
      <dd className="fact-value">{children}</dd>
    </div>
  );
}
