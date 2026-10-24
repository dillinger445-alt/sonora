/**
 * The rules of a smart playlist: which library tracks it lists and in what
 * order. Tempo, keys (on the Camelot wheel), genres, DJ score and text;
 * other filters saved from the library are kept as they are.
 */

import { useEffect, useMemo, useState } from 'react';
import type { LibraryFilter, SmartRules, Sort, SortKey } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Segmented } from '../components/Controls.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { KeyWheel } from '../components/KeyWheel.tsx';
import { formatCount, plural } from '../lib/format.ts';
import { expandCompatible, formatBpm, formatKey } from '../lib/keys.ts';
import { bpmMaxText, otherRules } from '../lib/rules.ts';
import { useCore } from '../state/core.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import { useToast } from '../state/toasts.tsx';

export type RulesTarget = { mode: 'create'; name: string; rules: SmartRules } | { mode: 'edit'; id: number };

const SORTS: { value: SortKey; label: string }[] = [
  { value: 'bpm', label: 'Tempo' },
  { value: 'key', label: 'Key' },
  { value: 'title', label: 'Title' },
  { value: 'artist', label: 'Artist' },
  { value: 'dj', label: 'DJ score' },
  { value: 'added', label: 'Date added' },
];

/** Names for the other orders, which arrive with filters saved from the library. */
const OTHER_SORTS: Record<SortKey, string> = {
  title: 'Title',
  artist: 'Artist',
  album: 'Album',
  duration: 'Duration',
  format: 'Format',
  sampleRate: 'Sample rate',
  bitDepth: 'Bit depth',
  bitrate: 'Bitrate',
  overall: 'Overall score',
  dj: 'DJ score',
  listening: 'Listening score',
  confidence: 'Confidence',
  lufs: 'Loudness',
  truePeak: 'True peak',
  cutoff: 'Cutoff',
  size: 'Size',
  added: 'Date added',
  analyzed: 'Date analyzed',
  path: 'Location',
  status: 'Status',
  bpm: 'Tempo',
  key: 'Key',
  genre: 'Genre',
};

/** The usual orders, plus `current` when it is another one, so the list always shows the real order. */
function sortChoices(current: SortKey): { value: SortKey; label: string }[] {
  return SORTS.some((s) => s.value === current) ? SORTS : [...SORTS, { value: current, label: OTHER_SORTS[current] ?? current }];
}

const SCORES: { value: string; label: string }[] = [
  { value: 'any', label: 'Any' },
  { value: '55', label: '55+' },
  { value: '75', label: '75+' },
  { value: '90', label: '90+' },
];

const EMPTY: SmartRules = { filter: {}, sort: { key: 'bpm', desc: false } };

function num(text: string): number | null {
  const t = text.trim().replace(',', '.');
  if (!t) return null;
  const v = Number(t);
  return Number.isFinite(v) ? v : null;
}

/** The "DJ score" choice a saved score filter shows as, or "any". */
function scoreChoice(f: LibraryFilter): string {
  return f.scoreKind === 'dj' && f.maxScore == null && f.minScore != null && SCORES.some((s) => s.value === String(f.minScore)) ? String(f.minScore) : 'any';
}

export function SmartRulesDialog({ target, onClose }: { target: RulesTarget | null; onClose: () => void }) {
  const { client, settings } = useCore();
  const pl = usePlaylists();
  const toast = useToast();
  const open = target != null;
  const editing = target?.mode === 'edit' ? pl.byId.get(target.id) : undefined;
  const initial: SmartRules = target?.mode === 'create' ? target.rules : (editing?.rules ?? EMPTY);
  const initialName = target?.mode === 'create' ? target.name : (editing?.name ?? '');
  const initialScore = scoreChoice(initial.filter);
  const notation = settings?.keyNotation ?? 'camelot';

  const [name, setName] = useState('');
  const [search, setSearch] = useState('');
  const [bpmMin, setBpmMin] = useState('');
  const [bpmMax, setBpmMax] = useState('');
  const [halfDouble, setHalfDouble] = useState(false);
  const [keys, setKeys] = useState<number[]>([]);
  const [mix, setMix] = useState(false);
  const [genres, setGenres] = useState<string[]>([]);
  const [score, setScore] = useState('any');
  const [sort, setSort] = useState<Sort>({ key: 'bpm', desc: false });
  const [limit, setLimit] = useState('');
  const [allGenres, setAllGenres] = useState<{ id: string; count: number }[]>([]);
  const [count, setCount] = useState<number | null>(null);
  // The upper tempo as it was opened: kept exactly unless edited here, so
  // filters saved from the library match what the library showed (B42).
  const [bpmMaxOpened, setBpmMaxOpened] = useState('');
  const [busy, setBusy] = useState(false);

  // A fresh form for every opening.
  const key = target ? (target.mode === 'edit' ? `e${target.id}` : `c${target.name}`) : '';
  useEffect(() => {
    if (!target) return;
    const f = initial.filter;
    setName(initialName);
    setSearch(f.search ?? '');
    setBpmMin(f.bpmMin != null ? formatBpm(f.bpmMin) : '');
    // "to 125" is saved as 125.999 (so 125.9 is in): show it as typed.
    const maxText = f.bpmMax != null ? bpmMaxText(f.bpmMax) : '';
    setBpmMax(maxText);
    setBpmMaxOpened(maxText);
    // The count of the playlist opened last is not this one's (B94).
    setCount(null);
    setHalfDouble(!!f.bpmHalfDouble);
    setKeys(f.keys ?? []);
    setMix(false);
    setGenres(f.genres ?? []);
    setScore(scoreChoice(f));
    setSort(initial.sort ?? { key: 'bpm', desc: false });
    setLimit(initial.limit ? String(initial.limit) : '');
    setBusy(false);
    client.facets().then(
      (fc) => setAllGenres(fc.genres.slice(0, 60)),
      () => setAllGenres([]),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  const lo = num(bpmMin);
  const hi = num(bpmMax);
  const lim = num(limit);
  const bpmBad = (lo != null && (lo < 20 || lo > 400)) || (hi != null && (hi < 20 || hi > 400)) || (lo != null && hi != null && lo > hi);
  const limitBad = lim != null && (!Number.isInteger(lim) || lim < 1 || lim > 10000);
  const matchedKeys = useMemo(() => (mix ? expandCompatible(keys) : keys), [keys, mix]);

  const rules = useMemo<SmartRules>(() => {
    const f: LibraryFilter = { ...initial.filter };
    delete f.search;
    delete f.bpmMin;
    delete f.bpmMax;
    delete f.bpmHalfDouble;
    delete f.keys;
    delete f.genres;
    // The score range saved with it stays as it was until a DJ score is
    // chosen here, which replaces it (a filter holds one score range).
    if (score !== initialScore) {
      delete f.scoreKind;
      delete f.minScore;
      delete f.maxScore;
      if (score !== 'any') {
        f.scoreKind = 'dj';
        f.minScore = Number(score);
      }
    }
    if (search.trim()) f.search = search.trim();
    if (lo != null) f.bpmMin = lo;
    // "to 125" includes 125.9; a range opened here and left alone stays as it was.
    if (hi != null) f.bpmMax = bpmMax === bpmMaxOpened && initial.filter.bpmMax != null ? initial.filter.bpmMax : Number.isInteger(hi) ? hi + 0.999 : hi;
    if ((lo != null || hi != null) && halfDouble) f.bpmHalfDouble = true;
    if (matchedKeys.length) f.keys = matchedKeys;
    if (genres.length) f.genres = genres;
    return { filter: f, sort, limit: lim != null && !limitBad ? lim : null };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, search, lo, hi, halfDouble, matchedKeys, genres, score, sort, lim, limitBad, bpmMax, bpmMaxOpened]);

  // How many tracks match, as the rules change.
  const rulesKey = JSON.stringify(rules);
  useEffect(() => {
    if (!open || bpmBad) return;
    let stale = false;
    const t = setTimeout(() => {
      client.query(rules.filter, rules.sort, 0, 1).then(
        (r) => {
          if (!stale) setCount(rules.limit ? Math.min(r.total, rules.limit) : r.total);
        },
        () => {
          if (!stale) setCount(null);
        },
      );
    }, 200);
    return () => {
      stale = true;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, rulesKey, bpmBad]);

  const save = async () => {
    const n = name.trim();
    if (!n) return;
    setBusy(true);
    if (target?.mode === 'create') {
      const node = await pl.createSmart(n, rules);
      if (node) onClose();
      else setBusy(false);
      return;
    }
    if (target?.mode === 'edit') {
      try {
        await client.updatePlaylist(target.id, { name: n, rules });
        onClose();
      } catch (e) {
        toast({ tone: 'error', title: 'Could not save the rules', body: e instanceof Error ? e.message : String(e) });
        setBusy(false);
      }
    }
  };

  const toggleKey = (k: number) => setKeys(keys.includes(k) ? keys.filter((x) => x !== k) : [...keys, k]);
  const kept = otherRules(initial.filter, score === initialScore && initialScore === 'any');
  // The most common genres, and every chosen one even when it is not among
  // them, so a rule can always be seen and turned off (B41).
  const genreChips: { id: string; count: number | null }[] = [
    ...allGenres,
    ...genres.filter((g) => !allGenres.some((x) => x.id.toLowerCase() === g.toLowerCase())).map((id) => ({ id, count: null })),
  ];
  const selected = new Set(keys);
  const related = new Set(mix ? matchedKeys.filter((k) => !selected.has(k)) : []);

  return (
    <Dialog
      open={open}
      title={target?.mode === 'edit' ? 'Edit smart playlist' : 'New smart playlist'}
      onClose={onClose}
      width={660}
      footer={
        <>
          <span className="rules-count faint" aria-live="polite">
            {bpmBad ? <span className="t-amber">Tempos run from 20 to 400 BPM, lowest first.</span> : count == null ? '' : `${plural(count, 'track')} ${count === 1 ? 'matches' : 'match'} now`}
          </span>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} disabled={!name.trim() || bpmBad || limitBad} onClick={() => void save()}>
            {target?.mode === 'edit' ? 'Save' : 'Make smart playlist'}
          </Button>
        </>
      }
    >
      <form
        className="rules"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <p className="dialog-text">A smart playlist lists the library's tracks that match, and keeps up as tracks are added, analyzed or edited.</p>
        <label className="field">
          <span className="field-label">Name</span>
          <input className="text-input" value={name} maxLength={120} onChange={(e) => setName(e.target.value)} data-autofocus />
        </label>
        <div className="rules-grid">
          <div className="rules-col">
            <label className="field">
              <span className="field-label">Words in title, artist, album, genre or file name</span>
              <input className="text-input" value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Any" />
            </label>
            <div className="field">
              <span className="field-label" id="rules-bpm">
                Tempo
              </span>
              <div className="bpm-inputs" role="group" aria-labelledby="rules-bpm">
                <input className="text-input num" inputMode="decimal" placeholder="From" aria-label="Lowest BPM" value={bpmMin} onChange={(e) => setBpmMin(e.target.value)} />
                <span className="faint">to</span>
                <input className="text-input num" inputMode="decimal" placeholder="To" aria-label="Highest BPM" value={bpmMax} onChange={(e) => setBpmMax(e.target.value)} />
                <span className="faint">BPM</span>
              </div>
              <label className="check-row">
                <input type="checkbox" className="check" checked={halfDouble} onChange={(e) => setHalfDouble(e.target.checked)} />
                <span>Also half and double tempo</span>
              </label>
            </div>
            <div className="field">
              <span className="field-label">DJ score</span>
              <Segmented label="DJ score" size="sm" value={score} onChange={setScore} options={SCORES} />
            </div>
            <div className="field">
              <span className="field-label">Order</span>
              <div className="rules-order">
                <select className="select" aria-label="Order by" value={sort.key} onChange={(e) => setSort({ ...sort, key: e.target.value as SortKey })}>
                  {sortChoices(sort.key).map((s) => (
                    <option key={s.value} value={s.value}>
                      {s.label}
                    </option>
                  ))}
                </select>
                <Segmented
                  label="Direction"
                  size="sm"
                  value={sort.desc ? 'desc' : 'asc'}
                  onChange={(v) => setSort({ ...sort, desc: v === 'desc' })}
                  options={[
                    { value: 'asc', label: 'Rising' },
                    { value: 'desc', label: 'Falling' },
                  ]}
                />
              </div>
            </div>
            <label className="field">
              <span className="field-label">At most</span>
              <span className="rules-limit">
                <input className={`text-input num ${limitBad ? 'is-invalid' : ''}`} inputMode="numeric" placeholder="All" value={limit} onChange={(e) => setLimit(e.target.value)} aria-label="At most this many tracks" />
                <span className="faint">tracks</span>
              </span>
            </label>
          </div>
          <div className="rules-col">
            <div className="field">
              <span className="field-label">Keys {keys.length ? <span className="faint">({keys.map((k) => formatKey(k, notation)).join(', ')})</span> : <span className="faint">(any)</span>}</span>
              <KeyWheel selected={selected} related={related} notation={notation} onToggle={toggleKey} label="Keys" size={208} />
              <label className="check-row">
                <input type="checkbox" className="check" checked={mix} disabled={keys.length === 0} onChange={(e) => setMix(e.target.checked)} />
                <span>Include keys that mix with these</span>
              </label>
            </div>
          </div>
        </div>
        {genreChips.length ? (
          <div className="field">
            <span className="field-label">Genres {genres.length ? <span className="faint">({genres.join(', ')})</span> : <span className="faint">(any)</span>}</span>
            <div className="genre-chips" role="group" aria-label="Genres">
              {genreChips.map((g) => {
                const on = genres.some((x) => x.toLowerCase() === g.id.toLowerCase());
                return (
                  <button
                    key={g.id}
                    type="button"
                    className={`genre-chip ${on ? 'is-on' : ''}`}
                    aria-pressed={on}
                    onClick={() => setGenres(on ? genres.filter((x) => x.toLowerCase() !== g.id.toLowerCase()) : [...genres, g.id])}
                  >
                    {g.id} {g.count != null ? <span className="num faint">{formatCount(g.count)}</span> : null}
                  </button>
                );
              })}
            </div>
          </div>
        ) : null}
        {kept.length ? <p className="faint rules-kept">Also from the library's filters: {kept.join(', ')}.</p> : null}
        {/* Enter in a field saves (a rendered button: WebKit needs one with several fields). */}
        <button type="submit" className="visually-hidden" aria-hidden tabIndex={-1} disabled={!name.trim() || bpmBad || limitBad || busy} />
      </form>
    </Dialog>
  );
}
