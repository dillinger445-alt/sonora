/**
 * Long-term average spectrum on a logarithmic frequency axis: the average
 * power of both channels (the engine's main spectrum; unlike a mid signal it
 * cannot cancel out when one channel is inverted) and the side signal.
 */

import { useMemo } from 'react';
import type uPlot from 'uplot';
import type { Visuals } from '../api/types.ts';
import { formatHz } from '../lib/format.ts';
import { cssVar } from './canvas.ts';
import { UPlotChart } from './UPlotChart.tsx';

const FREQ_TICKS = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000, 40000];

export function SpectrumChart({ spectrum, cutoffHz, nyquist }: { spectrum: Visuals['spectrum']; cutoffHz: number | null; nyquist: number }) {
  const data = useMemo<uPlot.AlignedData>(() => {
    const hz: number[] = [];
    const mid: (number | null)[] = [];
    const side: (number | null)[] = [];
    spectrum.hz.forEach((f, i) => {
      if (f <= 0) return;
      hz.push(f);
      mid.push(spectrum.mid[i] ?? null);
      side.push(spectrum.side ? (spectrum.side[i] ?? null) : null);
    });
    return spectrum.side ? [hz, mid, side] : [hz, mid];
  }, [spectrum]);

  const options = useMemo<Omit<uPlot.Options, 'width' | 'height'>>(() => {
    const text3 = cssVar('--c-text-3', '#8c949d');
    const grid = cssVar('--c-grid', '#2a2f35');
    const d1 = cssVar('--c-data-1', '#9fb7cc');
    const d2 = cssVar('--c-data-2', '#6f8aa3');
    const amber = cssVar('--c-amber', '#e9ae40');
    const font = "10px 'Martian Mono', ui-monospace, monospace";
    const mids = (data[1] as (number | null)[]).filter((v): v is number => v != null);
    const top = mids.length ? Math.ceil((Math.max(...mids) + 6) / 10) * 10 : 0;
    const series: uPlot.Series[] = [
      { label: 'Frequency', value: (_u, v) => (v == null ? '—' : formatHz(v)) },
      { label: 'Both channels', stroke: d1, width: 1.5, value: (_u, v) => (v == null ? '—' : `${v.toFixed(1)} dB`) },
    ];
    if (data.length > 2) series.push({ label: 'Side (L−R)', stroke: d2, width: 1, dash: [4, 3], value: (_u, v) => (v == null ? '—' : `${v.toFixed(1)} dB`) });
    return {
      scales: {
        x: { distr: 3, log: 10, range: [20, Math.max(1000, nyquist)] },
        y: { range: [top - 100, top] },
      },
      axes: [
        {
          stroke: text3,
          font,
          grid: { stroke: grid, width: 1 },
          ticks: { stroke: grid, width: 1, size: 4 },
          splits: () => FREQ_TICKS.filter((f) => f <= nyquist),
          // Keep every tick (uPlot thins log-axis labels to decades by default).
          filter: (_u, splits) => splits,
          values: (_u, splits) => splits.map((f) => (f == null ? '' : f >= 1000 ? `${f / 1000}k` : String(f))),
        },
        {
          stroke: text3,
          font,
          size: 44,
          grid: { stroke: grid, width: 1 },
          ticks: { stroke: grid, width: 1, size: 4 },
          values: (_u, splits) => splits.map((v) => (v == null ? '' : String(v))),
        },
      ],
      series,
      legend: { show: true, live: true },
      cursor: { drag: { x: false, y: false }, points: { size: 6 } },
      hooks: {
        draw: [
          (u: uPlot) => {
            if (!cutoffHz || cutoffHz >= nyquist) return;
            const x = Math.round(u.valToPos(cutoffHz, 'x', true)) + 0.5;
            const ctx = u.ctx;
            ctx.save();
            ctx.strokeStyle = amber;
            ctx.setLineDash([4, 3]);
            ctx.lineWidth = devicePixelRatio || 1;
            ctx.beginPath();
            ctx.moveTo(x, u.bbox.top);
            ctx.lineTo(x, u.bbox.top + u.bbox.height);
            ctx.stroke();
            ctx.restore();
          },
        ],
      },
    };
  }, [data, cutoffHz, nyquist]);

  return <UPlotChart options={options} data={data} height={210} label="Average spectrum of the mid (and side) signal, in dB, on a logarithmic frequency axis" />;
}

/** Share of energy per frequency band. */
export function BandBars({ bands }: { bands: { id: string; label: string; loHz: number; hiHz: number; share: number; relDb: number }[] }) {
  const max = Math.max(0.0001, ...bands.map((b) => b.share));
  return (
    <ul className="bands" aria-label="Energy by frequency band">
      {bands.map((b) => (
        <li key={b.id} className="band-row">
          <span className="band-label" data-tip={`${formatHz(b.loHz)} to ${formatHz(b.hiHz)}`}>
            {b.label}
          </span>
          <span className="band-track" aria-hidden>
            <span className="band-fill" style={{ width: `${(b.share / max) * 100}%` }} />
          </span>
          <span className="num band-share">{b.share >= 0.001 ? `${(b.share * 100).toFixed(1)}%` : '<0.1%'}</span>
        </li>
      ))}
    </ul>
  );
}
