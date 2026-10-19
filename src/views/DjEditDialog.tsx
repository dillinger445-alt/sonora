/**
 * Corrects the tempo, key and genre SONORA shows for one or more tracks.
 * Edits live in the library database; the audio files are never changed
 * (a DJ export writes the values into the exported copies).
 */

import { useEffect, useId, useMemo, useState } from 'react';
import type { TrackRow } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { SourceNote } from '../components/DjMarks.tsx';
import { KeyWheel } from '../components/KeyWheel.tsx';
import { plural } from '../lib/format.ts';
import { formatBpm, formatKey, longKeyName, parseKey } from '../lib/keys.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';

export interface DjEditTarget {
  ids: number[];
  /** The track, when only one is edited (its values fill the form). */
  row?: TrackRow;
}

function parseBpm(text: string): number | null | 'invalid' {
  const t = text.trim().replace(',', '.');
  if (!t) return null;
  const v = Number(t);
  return Number.isFinite(v) && v >= 20 && v <= 400 ? v : 'invalid';
}

export function DjEditDialog({ target, onClose }: { target: DjEditTarget | null; onClose: () => void }) {
  const { client, settings } = useCore();
  const toast = useToast();
  const notation = settings?.keyNotation ?? 'camelot';
  const row = target?.row && target.ids.length === 1 ? target.row : undefined;
  const many = (target?.ids.length ?? 0) > 1;
  const [bpmText, setBpmText] = useState('');
  const [factor, setFactor] = useState<0.5 | 2 | null>(null);
  const [keyText, setKeyText] = useState('');
  const [genre, setGenre] = useState('');
  const [busy, setBusy] = useState(false);
  const [genres, setGenres] = useState<string[]>([]);
  const listId = useId();

  // Fresh form for every target.
  const targetKey = target ? target.ids.join(',') : '';
  useEffect(() => {
    if (!target) return;
    setBpmText(row?.djBpm != null ? formatBpm(row.djBpm) : '');
    setFactor(null);
    setKeyText(row?.djKey != null ? formatKey(row.djKey, notation) : '');
    setGenre(row?.djGenre ?? '');
    setBusy(false);
    client.facets().then(
      (f) => setGenres(f.genres.map((g) => g.id)),
      () => setGenres([]),
    );
    // Only a new target resets the form.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [targetKey]);

  const bpm = parseBpm(bpmText);
  const key = keyText.trim() ? parseKey(keyText) : null;
  const keyInvalid = keyText.trim() !== '' && key == null;
  const selectedKeys = useMemo(() => new Set(key != null ? [key] : []), [key]);

  if (!target) return null;

  const initial = {
    bpm: row?.djBpm != null ? formatBpm(row.djBpm) : '',
    key: row?.djKey ?? null,
    genre: row?.djGenre ?? '',
  };
  const edit: { bpm?: number | null; bpmFactor?: 0.5 | 2; key?: number | null; genre?: string | null } = {};
  if (factor) edit.bpmFactor = factor;
  else if (bpm !== 'invalid' && (many ? bpm != null : bpmText.trim() !== initial.bpm)) edit.bpm = bpm;
  if (!keyInvalid && (many ? key != null : key !== initial.key)) edit.key = key;
  if (many ? genre.trim() !== '' : genre.trim() !== initial.genre) edit.genre = genre.trim() || null;
  const changed = Object.keys(edit).length > 0;
  const invalid = bpm === 'invalid' || keyInvalid;

  const save = async (reset = false) => {
    setBusy(true);
    try {
      const r = await client.editDj(target.ids, reset ? { reset: true } : edit);
      toast({ tone: 'success', title: reset ? `Reset ${plural(r.updated, 'track')} to detected values` : `Updated ${plural(r.updated, 'track')}` });
      onClose();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not save the changes', body: e instanceof Error ? e.message : String(e) });
      setBusy(false);
    }
  };

  const scale = (f: 0.5 | 2) => {
    if (many) {
      setFactor(factor === f ? null : f);
      setBpmText('');
      return;
    }
    if (typeof bpm === 'number') setBpmText(formatBpm(bpm * f));
  };

  return (
    <Dialog
      open
      title={many ? `Edit ${plural(target.ids.length, 'track')}` : 'Edit BPM, key and genre'}
      onClose={onClose}
      width={600}
      footer={
        <>
          <Button variant="ghost" className="dj-reset" disabled={busy} onClick={() => void save(true)} data-tip="Removes your edits and Discogs genres: detected values and tags apply again">
            Reset to detected
          </Button>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" disabled={!changed || invalid} busy={busy} onClick={() => void save()}>
            Save
          </Button>
        </>
      }
    >
      <form
        className="dj-edit"
        onSubmit={(e) => {
          e.preventDefault();
          if (changed && !invalid) void save();
        }}
      >
        {row ? <p className="dj-edit-title selectable">{row.displayTitle}</p> : null}
        <p className="dialog-text dj-edit-note">
          SONORA keeps these values in its library; your files are not changed. A DJ export writes them into the exported copies.
          {many ? ' Fields left empty keep each track’s own value.' : ''}
        </p>
        <div className="dj-edit-grid">
          <div className="dj-edit-fields">
            <div className="field">
              <label className="field-label" htmlFor={`${listId}-bpm`}>
                Tempo {row ? <SourceNote source={row.djBpmSource} /> : null}
              </label>
              <div className="bpm-edit">
                <input
                  id={`${listId}-bpm`}
                  className={`text-input num ${bpm === 'invalid' ? 'is-invalid' : ''}`}
                  inputMode="decimal"
                  placeholder={many ? (factor ? (factor === 2 ? 'Doubling every tempo' : 'Halving every tempo') : 'Keep each track’s tempo') : 'Not known'}
                  value={bpmText}
                  aria-invalid={bpm === 'invalid'}
                  onChange={(e) => {
                    setBpmText(e.target.value);
                    setFactor(null);
                  }}
                  data-autofocus
                />
                <span className="faint">BPM</span>
                <Button size="sm" aria-pressed={many ? factor === 2 : undefined} className={factor === 2 ? 'is-filtered' : ''} disabled={!many && typeof bpm !== 'number'} onClick={() => scale(2)} data-tip="Double the tempo (a half-time reading)">
                  ×2
                </Button>
                <Button size="sm" aria-pressed={many ? factor === 0.5 : undefined} className={factor === 0.5 ? 'is-filtered' : ''} disabled={!many && typeof bpm !== 'number'} onClick={() => scale(0.5)} data-tip="Halve the tempo (a double-time reading)">
                  ÷2
                </Button>
              </div>
              {bpm === 'invalid' ? <p className="field-error">Enter a tempo between 20 and 400 BPM.</p> : null}
              {row?.bpmDetected != null && row.djBpmSource !== 'detected' ? (
                <p className="field-hint faint">
                  Detected: <span className="num">{formatBpm(row.bpmDetected)}</span> BPM
                </p>
              ) : null}
            </div>
            <div className="field">
              <label className="field-label" htmlFor={`${listId}-key`}>
                Key {row ? <SourceNote source={row.djKeySource} /> : null}
              </label>
              <input
                id={`${listId}-key`}
                className={`text-input ${keyInvalid ? 'is-invalid' : ''}`}
                placeholder={many ? 'Keep each track’s key' : 'Not known'}
                value={keyText}
                aria-invalid={keyInvalid}
                aria-describedby={`${listId}-key-hint`}
                spellCheck={false}
                onChange={(e) => setKeyText(e.target.value)}
              />
              <p id={`${listId}-key-hint`} className={`field-hint ${keyInvalid ? 't-amber' : 'faint'}`}>
                {keyInvalid ? 'Not a key. Try 8A, Am, A minor or 1m.' : key != null ? `${longKeyName(key)}: ${formatKey(key, 'camelot')}, ${formatKey(key, 'openkey')}` : 'Type it in any notation, or pick it on the wheel.'}
              </p>
              {row?.keyDetected != null && row.djKeySource !== 'detected' ? (
                <p className="field-hint faint">Detected: {formatKey(row.keyDetected, notation)}</p>
              ) : null}
            </div>
            <div className="field">
              <label className="field-label" htmlFor={`${listId}-genre`}>
                Genre {row ? <SourceNote source={row.djGenreSource} /> : null}
              </label>
              <input
                id={`${listId}-genre`}
                className="text-input dj-genre-input"
                placeholder={many ? 'Keep each track’s genre' : 'None'}
                value={genre}
                maxLength={80}
                list={`${listId}-genres`}
                onChange={(e) => setGenre(e.target.value)}
              />
              <datalist id={`${listId}-genres`}>
                {genres.map((g) => (
                  <option key={g} value={g} />
                ))}
              </datalist>
            </div>
          </div>
          <KeyWheel
            size={208}
            selected={selectedKeys}
            notation={notation}
            label="Key"
            onToggle={(k) => setKeyText(key === k ? '' : formatKey(k, notation))}
          />
        </div>
        {/* Enter in a field saves. */}
        <button type="submit" hidden aria-hidden tabIndex={-1} />
      </form>
    </Dialog>
  );
}
