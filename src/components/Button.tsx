import { forwardRef, type ButtonHTMLAttributes, type ComponentType, type ReactNode } from 'react';

type IconComponent = ComponentType<{ className?: string; 'aria-hidden'?: boolean }>;

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: 'primary' | 'secondary' | 'ghost' | 'danger';
  size?: 'sm' | 'md';
  icon?: IconComponent;
  /** Trailing element, e.g. a chevron or a count. */
  trailing?: ReactNode;
  busy?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { variant = 'secondary', size = 'md', icon: Icon, trailing, busy, className, children, disabled, type = 'button', ...rest },
  ref,
) {
  const cls = ['btn', `btn-${variant}`, size === 'sm' ? 'btn-sm' : '', busy ? 'is-busy' : '', className ?? ''].filter(Boolean).join(' ');
  return (
    <button ref={ref} type={type} className={cls} disabled={disabled || busy} aria-busy={busy || undefined} {...rest}>
      {Icon ? <Icon className="btn-icon" aria-hidden /> : null}
      {children != null ? <span className="btn-label">{children}</span> : null}
      {trailing}
    </button>
  );
});

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Accessible name, also shown as a tooltip. */
  label: string;
  icon: IconComponent;
  size?: 'sm' | 'md';
  tone?: 'default' | 'danger';
  pressed?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton(
  { label, icon: Icon, size = 'md', tone = 'default', pressed, className, type = 'button', ...rest },
  ref,
) {
  const cls = ['icon-btn', size === 'sm' ? 'icon-btn-sm' : '', tone === 'danger' ? 'icon-btn-danger' : '', className ?? ''].filter(Boolean).join(' ');
  return (
    <button ref={ref} type={type} className={cls} aria-label={label} data-tip={label} aria-pressed={pressed} {...rest}>
      <Icon aria-hidden />
    </button>
  );
});
