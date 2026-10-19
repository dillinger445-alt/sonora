/** Library filters for DJs: a tempo range and keys picked on the Camelot wheel. */

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuChevronDown } from 'react-icons/lu';
import type { KeyNotation } from '../api/types.ts';
import { expandCompatible, formatBpm, formatKey, longKeyName, parseKey } from '../lib/keys.ts';
import { Button } from './Button.tsx';
import { KeyWheel } from './KeyWheel.tsx';
import { Popover, type PopoverTriggerProps } from './Popover.tsx';

export interface BpmRange {
  min: number | null;
  max: number | null;
  /** Also match half and double the range. */
  halfDouble: boolean;
}

export const NO_BPM: BpmRange = { min: null, max: null, halfDouble: false };

export function bpmActive(r: BpmRange): boolean {
  return r.min != null || r.max != null;
}

function FilterTrigger({ p, text, chip }: { p: PopoverTriggerProps; text: string; chip: string | null }) {
  return (
    <Button size="sm" variant={chip ? 'secondary' : 'ghost'} className={chip ? 'is-filtered' : ''} trailing={<LuChevronDown className="btn-chevron" aria-hidden />} {...p}>
      {text}
      {chip ? <span className="btn-count num">{chip}</span> : null}
    </Button>
  );
}

/** A number typed by hand: kept as text while editing, reported once it parses. */
function useNumberText(value: number | null, onValue: (v: number | null) => void): [string, (t: string) => void, boolean] {
  const [text, setText] = useState(value == null ? '' : formatBpm(value));
  const [last, setLast] = useState(value);
  if (value !== last) {
    setLast(value);
    setText(value == null ? '' : formatBpm(value));
  }
  const parsed = text.trim() === '' ? null : Number(text.replace(',', '.'));
  const valid = parsed == null || (Number.isFinite(parsed) && parsed >= 20 && parsed <= 400);
  useEffect(() => {
    if (!valid || parsed === value) return;
    const t = setTimeout(() => onValue(parsed), 300);
    return () => clearTimeout(t);
    // `onValue` changes on every render of the parent; the text drives this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);
  return [text, setText, valid];
}

export function BpmFilter({ value, onChange, playingBpm }: { value: BpmRange; onChange: (v: BpmRange) => void; playingBpm: number | null }) {
  // Typed numbers arrive after a pause; merge them into the range as it is then.
  const latest = useRef(value);
  useLayoutEffect(() => {
    latest.current = value;
  });
  const [minText, setMinText, minOk] = useNumberText(value.min, (min) => onChange({ ...latest.current, min }));
  const [maxText, setMaxText, maxOk] = useNumberText(value.max, (max) => onChange({ ...latest.current, max }));
  const active = bpmActive(value);
  const chip = active ? `${value.min != null ? formatBpm(value.min) : '…'}–${value.max != null ? formatBpm(value.max) : '…'}` : null;
  const lo = value.min ?? 0;
  const hi = value.max;
  const near = (pct: number) => {
    if (playingBpm == null) return;
    onChange({ min: Math.round(playingBpm * (1 - pct) * 10) / 10, max: Math.round(playingBpm * (1 + pct) * 10) / 10, halfDouble: value.halfDouble });
  };
  return (
    <Popover label="Tempo filter" width={276} trigger={(p) => <FilterTrigger p={p} text="BPM" chip={chip} />}>
      {(close) => (
        <div className="pop-body">
          <div className="pop-title">Tempo</div>
          <div className="bpm-inputs">
            <label className="bpm-field">
              <span className="visually-hidden">Lowest BPM</span>
              <input
                className={`text-input num ${minOk ? '' : 'is-invalid'}`}
                inputMode="decimal"
                placeholder="From"
                value={minText}
                aria-invalid={!minOk}
                onChange={(e) => setMinText(e.target.value)}
                data-autofocus
              />
            </label>
            <span className="faint">to</span>
            <label className="bpm-field">
              <span className="visually-hidden">Highest BPM</span>
              <input
                className={`text-input num ${maxOk ? '' : 'is-invalid'}`}
                inputMode="decimal"
                placeholder="To"
                value={maxText}
                aria-invalid={!maxOk}
                onChange={(e) => setMaxText(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') close();
                }}
              />
            </label>
            <span className="faint">BPM</span>
          </div>
          {!minOk || !maxOk ? <p className="field-error">Tempos run from 20 to 400 BPM.</p> : null}
          <label className="check-row">
            <input type="checkbox" className="check" checked={value.halfDouble} onChange={(e) => onChange({ ...value, halfDouble: e.target.checked })} />
            <span>
              Also half and double tempo
              {active && value.halfDouble ? (
                <span className="faint num">
                  {' '}
                  ({formatBpm(lo / 2) || '0'}–{hi != null ? formatBpm(hi / 2) : '…'}, {formatBpm(lo * 2) || '0'}–{hi != null ? formatBpm(hi * 2) : '…'})
                </span>
              ) : null}
            </span>
          </label>
          {playingBpm != null ? (
            <div className="pop-row">
              <span className="faint">Near the track playing ({formatBpm(playingBpm)})</span>
              <span className="pop-chips">
                {[0.02, 0.04, 0.08].map((pct) => (
                  <button key={pct} type="button" className="time-chip num" onClick={() => near(pct)}>
                    ±{pct * 100}%
                  </button>
                ))}
              </span>
            </div>
          ) : null}
          <div className="pop-foot">
            <Button size="sm" variant="ghost" disabled={!active && !value.halfDouble} onClick={() => onChange(NO_BPM)}>
              Clear
            </Button>
          </div>
        </div>
      )}
    </Popover>
  );
}

export interface KeySelection {
  keys: number[];
  /** Also show keys that mix with the chosen ones. */
  mix: boolean;
}

export const NO_KEYS: KeySelection = { keys: [], mix: false };

/** The keys a selection matches. */
export function keysFor(sel: KeySelection): number[] {
  return sel.mix ? expandCompatible(sel.keys) : sel.keys;
}

export function KeyFilter({
  value,
  onChange,
  counts,
  notation,
  playingKey,
}: {
  value: KeySelection;
  onChange: (v: KeySelection) => void;
  counts: ReadonlyMap<number, number>;
  notation: KeyNotation;
  playingKey: number | null;
}) {
  const [text, setText] = useState('');
  const typedRef = useRef<HTMLInputElement>(null);
  const typed = text.trim() ? parseKey(text) : null;
  const selected = useMemo(() => new Set(value.keys), [value.keys]);
  const related = useMemo(() => new Set(value.mix ? expandCompatible(value.keys) : []), [value.keys, value.mix]);
  const toggle = (k: number) => onChange({ ...value, keys: selected.has(k) ? value.keys.filter((x) => x !== k) : [...value.keys, k] });
  const names = value.keys.map((k) => formatKey(k, notation));
  const chip = value.keys.length === 0 ? null : value.mix ? `${names.length === 1 ? names[0] : `${names.length} keys`} ±` : names.length <= 2 ? names.join(' ') : `${names.length} keys`;

  let hint: ReactNode = 'Any notation: 8A, Am, A minor, 1m';
  if (text.trim()) hint = typed == null ? <span className="t-amber">Not a key</span> : `${longKeyName(typed)}, ${formatKey(typed, 'camelot')} · ${formatKey(typed, 'openkey')}`;

  return (
    <Popover label="Key filter" width={292} trigger={(p) => <FilterTrigger p={p} text="Key" chip={chip} />}>
      {() => (
        <div className="pop-body">
          <KeyWheel selected={selected} related={related} counts={counts} notation={notation} onToggle={toggle} label="Keys" />
          <form
            className="key-type"
            onSubmit={(e) => {
              e.preventDefault();
              if (typed == null) return;
              if (!selected.has(typed)) onChange({ ...value, keys: [...value.keys, typed] });
              setText('');
              // The button disables itself; keep typing where you were.
              typedRef.current?.focus();
            }}
          >
            <input
              ref={typedRef}
              className="text-input"
              placeholder="Type a key"
              aria-label="Type a key"
              value={text}
              onChange={(e) => setText(e.target.value)}
              spellCheck={false}
            />
            <Button size="sm" type="submit" disabled={typed == null}>
              Add
            </Button>
          </form>
          <p className="key-type-hint faint">{hint}</p>
          <label className="check-row">
            <input type="checkbox" className="check" checked={value.mix} onChange={(e) => onChange({ ...value, mix: e.target.checked })} />
            <span>Include keys that mix with these (±1 on the wheel and the relative key)</span>
          </label>
          {playingKey != null ? (
            <div className="pop-row">
              <Button size="sm" onClick={() => onChange({ keys: [playingKey], mix: true })}>
                Mixes with the track playing ({formatKey(playingKey, notation)})
              </Button>
            </div>
          ) : null}
          <div className="pop-foot">
            <Button size="sm" variant="ghost" disabled={value.keys.length === 0 && !value.mix} onClick={() => onChange(NO_KEYS)}>
              Clear
            </Button>
          </div>
        </div>
      )}
    </Popover>
  );
}
