/**
 * The Camelot wheel as a key picker: minor keys (A) on the inner ring,
 * major keys (B) on the outer one, 12 at the top. Neighbours on the wheel
 * mix harmonically, so the wheel shows at a glance what fits together.
 *
 * Keyboard: the wheel is one Tab stop; ←/→ go round the wheel, ↑ moves to
 * the outer (major) ring and ↓ to the inner (minor) ring, Space or Enter
 * toggles the focused key.
 */

import { useMemo, useState, type KeyboardEvent } from 'react';
import type { KeyNotation } from '../api/types.ts';
import { plural } from '../lib/format.ts';
import { camelot, formatKey, fromCamelot, keyColor, longKeyName } from '../lib/keys.ts';

export interface KeyWheelProps {
  selected: ReadonlySet<number>;
  /** Keys shown as mixing with the selection (not selected themselves). */
  related?: ReadonlySet<number>;
  /** Tracks per key; keys without tracks are dimmed. */
  counts?: ReadonlyMap<number, number>;
  notation: KeyNotation;
  onToggle: (key: number) => void;
  label: string;
  size?: number;
}

/** A point at `r` from the centre, `deg` clockwise from 12 o'clock. */
function pt(c: number, r: number, deg: number): [number, number] {
  const a = (deg * Math.PI) / 180;
  return [c + r * Math.sin(a), c - r * Math.cos(a)];
}

function sector(c: number, r0: number, r1: number, a0: number, a1: number): string {
  const [x0, y0] = pt(c, r1, a0);
  const [x1, y1] = pt(c, r1, a1);
  const [x2, y2] = pt(c, r0, a1);
  const [x3, y3] = pt(c, r0, a0);
  const f = (v: number) => v.toFixed(2);
  return `M${f(x0)} ${f(y0)}A${r1} ${r1} 0 0 1 ${f(x1)} ${f(y1)}L${f(x2)} ${f(y2)}A${r0} ${r0} 0 0 0 ${f(x3)} ${f(y3)}Z`;
}

export function KeyWheel({ selected, related, counts, notation, onToggle, label, size = 236 }: KeyWheelProps) {
  const c = size / 2;
  const rim = 4;
  const rOuter = c - 1;
  const rMid = c * 0.69;
  const rInner = c * 0.4;
  const [focusKey, setFocusKey] = useState(() => fromCamelot(8, true) as number);
  const [hover, setHover] = useState<number | null>(null);
  const [hasFocus, setHasFocus] = useState(false);

  const segments = useMemo(
    () =>
      Array.from({ length: 24 }, (_, i) => {
        const n = (i % 12) + 1;
        const minor = i >= 12;
        const key = fromCamelot(n, minor) as number;
        const mid = (n % 12) * 30;
        const [r0, r1] = minor ? [rInner, rMid - 1] : [rMid + 1, rOuter - rim];
        const [lx, ly] = pt(c, (r0 + r1) / 2, mid);
        return { key, n, minor, path: sector(c, r0, r1, mid - 14.4, mid + 14.4), rim: minor ? null : sector(c, rOuter - rim + 0.5, rOuter, mid - 14.4, mid + 14.4), lx, ly };
      }),
    [c, rInner, rMid, rOuter],
  );

  const move = (key: number, e: KeyboardEvent) => {
    const { n, letter } = camelot(key);
    let next: number | null = null;
    if (e.key === 'ArrowRight') next = fromCamelot((n % 12) + 1, letter === 'A');
    else if (e.key === 'ArrowLeft') next = fromCamelot(((n + 10) % 12) + 1, letter === 'A');
    else if (e.key === 'ArrowUp') next = fromCamelot(n, false);
    else if (e.key === 'ArrowDown') next = fromCamelot(n, true);
    else if (e.key === 'Home') next = fromCamelot(1, letter === 'A');
    else if (e.key === 'End') next = fromCamelot(12, letter === 'A');
    else if (e.key === ' ' || e.key === 'Enter') {
      e.preventDefault();
      onToggle(key);
      return;
    }
    if (next == null) return;
    e.preventDefault();
    setFocusKey(next);
    (e.currentTarget.parentElement?.querySelector(`[data-key="${next}"]`) as SVGGElement | null)?.focus();
  };

  const only = selected.size === 1 ? ([...selected][0] ?? null) : null;
  const shown = hover ?? (hasFocus ? focusKey : only);
  const shownCount = shown != null ? (counts?.get(shown) ?? 0) : 0;

  return (
    <div className="wheel" style={{ width: size, height: size }}>
      <svg
        width={size}
        height={size}
        viewBox={`0 0 ${size} ${size}`}
        role="group"
        aria-label={label}
        onFocus={() => setHasFocus(true)}
        onBlur={() => setHasFocus(false)}
      >
        {segments.map((s) => {
          const on = selected.has(s.key);
          const rel = !on && !!related?.has(s.key);
          const count = counts?.get(s.key) ?? 0;
          const code = formatKey(s.key, notation);
          return (
            <g
              key={s.key}
              data-key={s.key}
              role="checkbox"
              aria-checked={on}
              aria-label={`${code}, ${longKeyName(s.key)}${counts ? `, ${plural(count, 'track')}` : ''}${rel ? ', mixes with the selection' : ''}`}
              tabIndex={s.key === focusKey ? 0 : -1}
              className={`wheel-seg ${on ? 'is-on' : ''} ${rel ? 'is-related' : ''} ${counts && count === 0 ? 'is-empty' : ''}`}
              onClick={() => {
                setFocusKey(s.key);
                onToggle(s.key);
              }}
              onKeyDown={(e) => move(s.key, e)}
              onPointerEnter={() => setHover(s.key)}
              onPointerLeave={() => setHover((h) => (h === s.key ? null : h))}
            >
              <path className="wheel-path" d={s.path} />
              {s.rim ? <path className="wheel-rim" d={s.rim} style={{ fill: keyColor(s.key) }} /> : null}
              <text className="wheel-code" x={s.lx} y={s.ly - (count > 0 ? 5 : 0)}>
                {code}
              </text>
              {count > 0 ? (
                <text className="wheel-count" x={s.lx} y={s.ly + 8}>
                  {count > 999 ? '999+' : count}
                </text>
              ) : null}
            </g>
          );
        })}
        <circle className="wheel-hub" cx={c} cy={c} r={rInner - 4} />
      </svg>
      <div className="wheel-center" aria-hidden>
        {shown != null ? (
          <>
            <span className="wheel-center-code" style={{ color: keyColor(shown) }}>
              {formatKey(shown, notation)}
            </span>
            <span className="wheel-center-name">{longKeyName(shown)}</span>
            {counts ? <span className="wheel-center-count num">{plural(shownCount, 'track')}</span> : null}
          </>
        ) : (
          <span className="wheel-center-name">{selected.size ? `${plural(selected.size, 'key')} chosen` : 'Any key'}</span>
        )}
      </div>
    </div>
  );
}
