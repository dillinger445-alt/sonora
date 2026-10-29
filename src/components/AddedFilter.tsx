/**
 * The library's Added filter: tracks added today, in the last 7, 30 or 90
 * days, this year, or between two days.
 */

import { ADDED_PERIODS, addedActive, addedChip, NO_ADDED, type AddedSelection } from '../lib/added.ts';
import { Button } from './Button.tsx';
import { FilterTrigger } from './DjFilters.tsx';
import { Popover } from './Popover.tsx';

export function AddedFilter({ value, onChange }: { value: AddedSelection; onChange: (v: AddedSelection) => void }) {
  return (
    <Popover label="Date added filter" width={300} trigger={(p) => <FilterTrigger p={p} text="Added" chip={addedChip(value)} />}>
      {(close) => (
        <div className="pop-body">
          <div className="pop-title">Added to SONORA</div>
          <div className="pop-chips added-periods" role="group" aria-label="Added within">
            {ADDED_PERIODS.map((p) => (
              <button
                key={p.id}
                type="button"
                className={`time-chip ${value.period === p.id ? 'is-on' : ''}`}
                aria-pressed={value.period === p.id}
                onClick={() => {
                  onChange(value.period === p.id ? NO_ADDED : { period: p.id, from: null, to: null });
                  close();
                }}
              >
                {p.label}
              </button>
            ))}
          </div>
          <div className="pop-title">Or between two days</div>
          <div className="added-days">
            <label className="added-day">
              <span className="faint">From</span>
              <input
                type="date"
                className="text-input num"
                aria-label="Added on or after"
                value={value.from ?? ''}
                max={value.to ?? undefined}
                onChange={(e) => onChange({ period: null, from: e.target.value || null, to: value.to })}
              />
            </label>
            <label className="added-day">
              <span className="faint">To</span>
              <input
                type="date"
                className="text-input num"
                aria-label="Added on or before"
                value={value.to ?? ''}
                min={value.from ?? undefined}
                onChange={(e) => onChange({ period: null, from: value.from, to: e.target.value || null })}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') close();
                }}
              />
            </label>
          </div>
          {addedActive(value) ? (
            <div className="pop-row">
              <Button size="sm" variant="ghost" onClick={() => onChange(NO_ADDED)}>
                Any time
              </Button>
            </div>
          ) : null}
        </div>
      )}
    </Popover>
  );
}
