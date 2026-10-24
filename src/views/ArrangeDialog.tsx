/**
 * Arranging a playlist for mixing: SONORA proposes a running order where
 * neighbours sit close on the Camelot wheel and in tempo; the set before
 * and after, side by side, and nothing changes until Apply.
 */

import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { ArrangeResult, KeyNotation, TrackRow } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { SetTimeline } from '../components/SetTimeline.tsx';
import { formatCount } from '../lib/format.ts';
import { useCore } from '../state/core.tsx';

function Stat({ label, before, after, better, format }: { label: string; before: number | null; after: number | null; better: 'up' | 'down'; format: (v: number) => string }) {
  const improved = before != null && after != null && (better === 'up' ? after > before : after < before);
  return (
    <div className="arr-stat">
      <dt>{label}</dt>
      <dd>
        <span className="num faint">{before != null ? format(before) : '–'}</span>
        <span className="arr-arrow" aria-hidden>
          →
        </span>
        <span className={`num ${improved ? 't-lime' : ''}`}>{after != null ? format(after) : '–'}</span>
      </dd>
    </div>
  );
}

export function ArrangeDialog({
  open,
  playlistId,
  rows,
  notation,
  onClose,
  onApply,
}: {
  open: boolean;
  playlistId: number;
  rows: TrackRow[];
  notation: KeyNotation;
  onClose: () => void;
  onApply: (order: number[]) => void;
}) {
  const { client } = useCore();
  const [keepFirst, setKeepFirst] = useState(true);
  const [rising, setRising] = useState(false);
  const [result, setResult] = useState<ArrangeResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const wrap = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(560);
  const first = rows[0];

  // Measured whenever it changes size: the dialog is not laid out yet when
  // it opens, so a measurement then reads 0 (B52).
  useLayoutEffect(() => {
    if (!open) return;
    const el = wrap.current;
    if (!el) return;
    const ro = new ResizeObserver(() => {
      if (el.clientWidth > 0) setWidth(el.clientWidth);
    });
    ro.observe(el);
    if (el.clientWidth > 0) setWidth(el.clientWidth);
    return () => ro.disconnect();
  }, [open]);

  useEffect(() => {
    if (!open) return;
    let stale = false;
    setError(null);
    // An order worked out for other choices must not be used meanwhile.
    setResult(null);
    client.arrangePlaylist(playlistId, { start: keepFirst ? (first?.id ?? null) : null, tempo: rising ? 'rising' : 'any' }).then(
      (r) => {
        if (!stale) setResult(r);
      },
      (e: unknown) => {
        if (!stale) setError(e instanceof Error ? e.message : String(e));
      },
    );
    return () => {
      stale = true;
    };
  }, [open, client, playlistId, keepFirst, rising, first?.id]);

  useEffect(() => {
    if (!open) setResult(null);
  }, [open]);

  const after = useMemo(() => {
    if (!result) return null;
    const by = new Map(rows.map((r) => [r.id, r]));
    return result.order.map((id) => by.get(id)).filter((r): r is TrackRow => r != null);
  }, [result, rows]);
  const same = !!result && result.order.every((id, i) => rows[i]?.id === id);

  return (
    <Dialog
      open={open}
      title="Arrange for mixing"
      onClose={onClose}
      width={680}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!result || same}
            onClick={() => {
              if (result) onApply(result.order);
              onClose();
            }}
            data-autofocus
          >
            {same ? 'Already in this order' : 'Use this order'}
          </Button>
        </>
      }
    >
      <div className="arr" ref={wrap}>
        <p className="dialog-text">
          Neighbours end up in keys that mix (the same key, the relative key or one step round the Camelot wheel) and close in tempo, half and double time included. Nothing changes
          until you use the order.
        </p>
        <div className="arr-options">
          <label className="check-row">
            <input type="checkbox" className="check" checked={keepFirst} onChange={(e) => setKeepFirst(e.target.checked)} />
            <span>Keep “{first?.displayTitle ?? 'the first track'}” first</span>
          </label>
          <label className="check-row">
            <input type="checkbox" className="check" checked={rising} onChange={(e) => setRising(e.target.checked)} />
            <span>Build up: slower tracks first</span>
          </label>
        </div>
        {error ? <p className="t-red">{error}</p> : null}
        <div className="arr-sets">
          <div>
            <span className="arr-label faint">Now</span>
            <SetTimeline rows={rows} notation={notation} width={width} compact label="The set in its current order" />
          </div>
          <div>
            <span className="arr-label">Arranged</span>
            {after ? (
              <SetTimeline rows={after} notation={notation} width={width} compact label="The set in the arranged order" />
            ) : error ? (
              <div className="arr-wait faint">No order: see above.</div>
            ) : (
              <div className="arr-wait faint">Working it out…</div>
            )}
          </div>
        </div>
        {result ? (
          <dl className="arr-stats">
            <Stat label={`Changes in mixing keys (of ${formatCount(result.after.transitions)})`} before={result.before.harmonic} after={result.after.harmonic} better="up" format={formatCount} />
            <Stat label="Key clashes" before={result.before.clashes} after={result.after.clashes} better="down" format={formatCount} />
            <Stat label="Average tempo change" before={result.before.tempoChange} after={result.after.tempoChange} better="down" format={(v) => `${v.toFixed(1)}%`} />
          </dl>
        ) : null}
      </div>
    </Dialog>
  );
}
