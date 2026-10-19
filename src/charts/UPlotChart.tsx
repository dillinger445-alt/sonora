/** Minimal React wrapper around uPlot (sized to its container). */

import { useEffect, useLayoutEffect, useRef } from 'react';
import uPlot from 'uplot';
import 'uplot/dist/uPlot.min.css';

export interface UPlotChartProps {
  /** Options without width/height (taken from the container). Memoize them. */
  options: Omit<uPlot.Options, 'width' | 'height'>;
  data: uPlot.AlignedData;
  height: number;
  label: string;
}

export function UPlotChart({ options, data, height, label }: UPlotChartProps) {
  const ref = useRef<HTMLDivElement>(null);
  const plot = useRef<uPlot | null>(null);
  const dataRef = useRef(data);
  useLayoutEffect(() => {
    dataRef.current = data;
  });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const u = new uPlot({ ...options, width: Math.max(50, el.clientWidth), height }, dataRef.current, el);
    plot.current = u;
    const ro = new ResizeObserver(() => u.setSize({ width: Math.max(50, el.clientWidth), height }));
    ro.observe(el);
    return () => {
      ro.disconnect();
      u.destroy();
      plot.current = null;
    };
  }, [options, height]);

  useEffect(() => {
    plot.current?.setData(data);
  }, [data]);

  return <div ref={ref} className="uplot-host" role="img" aria-label={label} />;
}
