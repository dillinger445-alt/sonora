/** Small form controls: segmented control, tabs, checkbox, search field, switch. */

import { forwardRef, useEffect, useRef, useState, type InputHTMLAttributes, type KeyboardEvent, type ReactNode } from 'react';
import { LuSearch, LuX } from 'react-icons/lu';
import { parseStepValue } from '../lib/stepper.ts';
import { formatCount } from '../lib/format.ts';

export interface SegmentedOption<T extends string> {
  value: T;
  label: string;
  hint?: string;
}

/** A radio group drawn as joined buttons (arrow keys move the selection). */
export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  size = 'md',
}: {
  value: T;
  options: SegmentedOption<T>[];
  onChange: (v: T) => void;
  label: string;
  size?: 'sm' | 'md';
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    const dir = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : e.key === 'ArrowLeft' || e.key === 'ArrowUp' ? -1 : 0;
    if (!dir) return;
    e.preventDefault();
    const next = (i + dir + options.length) % options.length;
    const opt = options[next];
    if (opt) {
      onChange(opt.value);
      refs.current[next]?.focus();
    }
  };
  return (
    <div className={`segmented ${size === 'sm' ? 'segmented-sm' : ''}`} role="radiogroup" aria-label={label}>
      {options.map((o, i) => {
        const checked = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={checked}
            tabIndex={checked ? 0 : -1}
            className={`segmented-item ${checked ? 'is-on' : ''}`}
            data-tip={o.hint}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

export interface TabItem<T extends string> {
  id: T;
  label: string;
  count?: number;
}

/** Id of a tab rendered by `TabList`, for the panel's `aria-labelledby`. */
export function tabId(idBase: string, id: string): string {
  return `${idBase}-tab-${id}`;
}

/**
 * Tabs that switch the panel below them (the ARIA tabs pattern): only the
 * selected tab is in the Tab order, and the arrow keys, Home and End move
 * between tabs. The panel carries `role="tabpanel"`, the id given as
 * `panelId` and `aria-labelledby={tabId(idBase, value)}`.
 */
export function TabList<T extends string>({
  items,
  value,
  onChange,
  label,
  idBase,
  panelId,
  variant = 'chips',
}: {
  items: TabItem<T>[];
  value: T;
  onChange: (v: T) => void;
  label: string;
  idBase: string;
  panelId: string;
  variant?: 'chips' | 'underline';
}) {
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const onKey = (e: KeyboardEvent, i: number) => {
    let next: number;
    if (e.key === 'ArrowRight') next = (i + 1) % items.length;
    else if (e.key === 'ArrowLeft') next = (i - 1 + items.length) % items.length;
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = items.length - 1;
    else return;
    e.preventDefault();
    const item = items[next];
    if (item) {
      onChange(item.id);
      refs.current[next]?.focus();
    }
  };
  const chip = variant === 'chips';
  return (
    <div className={chip ? 'chip-tabs' : 'cat-tabs'} role="tablist" aria-label={label}>
      {items.map((t, i) => {
        const on = t.id === value;
        return (
          <button
            key={t.id}
            ref={(el) => {
              refs.current[i] = el;
            }}
            id={tabId(idBase, t.id)}
            type="button"
            role="tab"
            aria-selected={on}
            aria-controls={panelId}
            tabIndex={on ? 0 : -1}
            className={`${chip ? 'chip-tab' : 'cat-tab'} ${on ? 'is-on' : ''}`}
            onClick={() => onChange(t.id)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {t.label}
            {t.count != null ? <span className="num cat-count">{formatCount(t.count)}</span> : null}
          </button>
        );
      })}
    </div>
  );
}

export interface CheckboxProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'type'> {
  indeterminate?: boolean;
}

export const Checkbox = forwardRef<HTMLInputElement, CheckboxProps>(function Checkbox({ indeterminate, className, ...rest }, ref) {
  const inner = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    if (inner.current) inner.current.indeterminate = !!indeterminate;
  }, [indeterminate]);
  return (
    <input
      ref={(el) => {
        inner.current = el;
        if (typeof ref === 'function') ref(el);
        else if (ref) ref.current = el;
      }}
      type="checkbox"
      className={`check ${className ?? ''}`}
      {...rest}
    />
  );
});

export const SearchField = forwardRef<
  HTMLInputElement,
  { value: string; onChange: (v: string) => void; placeholder: string; label: string; shortcut?: string; width?: number }
>(function SearchField({ value, onChange, placeholder, label, shortcut, width }, ref) {
  return (
    <div className="search" style={width ? { width } : undefined}>
      <LuSearch className="search-icon" aria-hidden />
      <input
        ref={ref}
        type="search"
        className="search-input"
        value={value}
        placeholder={placeholder}
        aria-label={label}
        spellCheck={false}
        autoComplete="off"
        onChange={(e) => onChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && value) {
            e.preventDefault();
            e.stopPropagation();
            onChange('');
          }
        }}
      />
      {value ? (
        <button type="button" className="search-clear" aria-label="Clear search" onClick={() => onChange('')}>
          <LuX aria-hidden />
        </button>
      ) : shortcut ? (
        <kbd className="search-kbd" aria-hidden>
          {shortcut}
        </kbd>
      ) : null}
    </div>
  );
});

/** An on/off switch for settings. */
export function Switch({
  checked,
  onChange,
  label,
  description,
  disabled,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  description?: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className={`switch-row ${disabled ? 'is-disabled' : ''}`}>
      <span className="switch-text">
        <span className="switch-label">{label}</span>
        {description ? <span className="switch-desc">{description}</span> : null}
      </span>
      <input type="checkbox" role="switch" className="switch" checked={checked} disabled={disabled} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

/** A determinate progress bar. Never animated without real progress. */
export function ProgressBar({ value, label, tone = 'lime', size = 'md' }: { value: number; label: string; tone?: 'lime' | 'amber' | 'neutral'; size?: 'sm' | 'md' }) {
  const v = Math.max(0, Math.min(1, Number.isFinite(value) ? value : 0));
  return (
    <div
      className={`progress progress-${tone} ${size === 'sm' ? 'progress-sm' : ''}`}
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(v * 100)}
    >
      <div className="progress-fill" style={{ width: `${v * 100}%` }} />
    </div>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="kbd">{children}</kbd>;
}

/** A number with − and + buttons; typed values are clamped to the range on
 *  Enter or blur. An emptied field goes back to the saved value (B46), and
 *  − and + step from what is typed or was just chosen, not from a value
 *  still being saved (B86). */
export function NumberStepper({ value, min, max, step = 1, onChange, label, unit }: { value: number; min: number; max: number; step?: number; onChange: (v: number) => void; label: string; unit?: string }) {
  const [text, setText] = useState(String(value));
  const [lastValue, setLastValue] = useState(value);
  if (value !== lastValue) {
    setLastValue(value);
    setText(String(value));
  }
  const current = parseStepValue(text, min, max, step) ?? value;
  const emit = (v: number) => {
    setText(String(v));
    if (v !== value) onChange(v);
  };
  const commit = (raw: string) => {
    const v = parseStepValue(raw, min, max, step);
    if (v == null) {
      setText(String(value));
      return;
    }
    emit(v);
  };
  return (
    <div className="stepper">
      <button type="button" className="stepper-btn" aria-label={`Decrease ${label}`} disabled={current <= min} onClick={() => emit(Math.max(min, current - step))}>
        −
      </button>
      <input
        className="stepper-input num"
        inputMode="numeric"
        aria-label={label}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={(e) => commit(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') commit((e.target as HTMLInputElement).value);
          if (e.key === 'Escape' && text !== String(value)) {
            e.preventDefault();
            setText(String(value));
          }
          if (e.key === 'ArrowUp') {
            e.preventDefault();
            emit(Math.min(max, current + step));
          }
          if (e.key === 'ArrowDown') {
            e.preventDefault();
            emit(Math.max(min, current - step));
          }
        }}
      />
      <button type="button" className="stepper-btn" aria-label={`Increase ${label}`} disabled={current >= max} onClick={() => emit(Math.min(max, current + step))}>
        +
      </button>
      {unit ? <span className="stepper-unit faint">{unit}</span> : null}
    </div>
  );
}
