/**
 * What a number typed into a stepper field means: the value clamped to the
 * range and rounded to the step, or `null` when there is no number (an
 * empty field goes back to the saved value instead of saving the minimum).
 */
export function parseStepValue(raw: string, min: number, max: number, step = 1): number | null {
  const t = raw.trim();
  if (t === '') return null;
  const v = Number(t);
  if (!Number.isFinite(v)) return null;
  return Math.max(min, Math.min(max, Math.round(v / step) * step));
}
