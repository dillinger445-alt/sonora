/**
 * Track browsing shared by "Music library" and "Needs attention":
 * search, filters, score view, the virtualized table, bulk actions and the
 * inspector panel.
 */

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { LuAudioLines, LuChevronDown, LuDownload, LuListFilter, LuPanelRightClose, LuPanelRightOpen, LuPencil, LuRefreshCw, LuTrash2, LuUsb, LuX } from 'react-icons/lu';
import type { LibraryFilter, ScoreKind, Sort, SortKey, TrackRow } from '../api/types.ts';
import { AddToPlaylist } from '../components/AddToPlaylist.tsx';
import { Button, IconButton } from '../components/Button.tsx';
import { SearchField, Segmented } from '../components/Controls.tsx';
import { BpmFilter, bpmActive, KeyFilter, keysFor, NO_BPM, NO_KEYS, type BpmRange, type KeySelection } from '../components/DjFilters.tsx';
import { Menu, type MenuItem } from '../components/Menu.tsx';
import { TrackTable, type TrackTableHandle } from '../components/TrackTable.tsx';
import { formatCount, plural } from '../lib/format.ts';
import { debounce, usePref } from '../lib/persist.ts';
import { navigate } from '../lib/router.ts';
import { formatBpm, formatKey } from '../lib/keys.ts';
import { flagInfo, SCORE_HINT, SCORE_KINDS, SCORE_LABEL } from '../lib/scores.ts';
import { useCore } from '../state/core.tsx';
import { usePlayer } from '../state/player.tsx';
import { usePlaylists } from '../state/playlists.tsx';
import { describeQueued, useTrackActions } from '../state/trackActions.tsx';
import { useToast } from '../state/toasts.tsx';
import { useResource } from '../state/useResource.ts';
import { Inspector } from './Inspector.tsx';

const SCORE_RANGES: { id: string; label: string; min?: number; max?: number }[] = [
  { id: 'excellent', label: 'Excellent (90–100)', min: 90 },
  { id: 'good', label: 'Good (75–89)', min: 75, max: 89.999 },
  { id: 'fair', label: 'Fair (55–74)', min: 55, max: 74.999 },
  { id: 'poor', label: 'Poor or worse (below 55)', max: 54.999 },
];

const STATUSES: { id: string; label: string }[] = [
  { id: 'done', label: 'Analyzed' },
  { id: 'pending', label: 'Waiting' },
  { id: 'analyzing', label: 'Analyzing' },
  { id: 'failed', label: 'Failed' },
  { id: 'missing', label: 'Missing file' },
];

const SORT_KEYS: SortKey[] = [
  'title',
  'artist',
  'album',
  'duration',
  'format',
  'sampleRate',
  'bitDepth',
  'bitrate',
  'overall',
  'dj',
  'listening',
  'confidence',
  'lufs',
  'truePeak',
  'cutoff',
  'size',
  'added',
  'analyzed',
  'path',
  'status',
  'bpm',
  'key',
  'genre',
];
const isSort = (v: unknown): v is Sort =>
  typeof v === 'object' && v !== null && SORT_KEYS.includes((v as Sort).key) && typeof (v as Sort).desc === 'boolean';
const isBool = (v: unknown): v is boolean => typeof v === 'boolean';

function toggle(list: string[], id: string): string[] {
  return list.includes(id) ? list.filter((x) => x !== id) : [...list, id];
}

export interface LibraryBrowserProps {
  /** Always applied (e.g. a "needs attention" category). */
  baseFilter?: LibraryFilter;
  prefsKey: string;
  defaultSort: Sort;
  showIssueFilter?: boolean;
  /** Shown when there are no tracks at all for the base filter. */
  emptyBase: ReactNode;
  label: string;
  /** Set when the browser is the panel of a tab list. */
  panel?: { id: string; labelledBy: string };
  /** Offer tempo and key detection for tracks analyzed before it existed. */
  offerDjDetection?: boolean;
}

export function LibraryBrowser({ baseFilter, prefsKey, defaultSort, showIssueFilter = true, emptyBase, label, panel, offerDjDetection = false }: LibraryBrowserProps) {
  const { client, settings, bridge } = useCore();
  const actions = useTrackActions();
  const player = usePlayer();
  const playlists = usePlaylists();
  const toast = useToast();
  const [bpm, setBpm] = useState<BpmRange>(NO_BPM);
  const [keySel, setKeySel] = useState<KeySelection>(NO_KEYS);
  const [genres, setGenres] = useState<string[]>([]);
  const [searchInput, setSearchInput] = useState('');
  const [search, setSearch] = useState('');
  const [formats, setFormats] = useState<string[]>([]);
  const [flags, setFlags] = useState<string[]>([]);
  const [statuses, setStatuses] = useState<string[]>([]);
  const [range, setRange] = useState<string | null>(null);
  const [scoreKind, setScoreKind] = useState<ScoreKind>(settings?.defaultScore ?? 'overall');
  const [sort, setSort] = usePref<Sort>(`${prefsKey}.sort`, defaultSort, isSort);
  const [inspectorOpen, setInspectorOpen] = usePref<boolean>('inspector', true, isBool);
  const [selection, setSelection] = useState<Set<number>>(new Set());
  const [active, setActive] = useState<TrackRow | null>(null);
  const [total, setTotal] = useState<number | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const tableRef = useRef<TrackTableHandle>(null);
  // A category with fixed statuses (Failed, Missing) keeps them: no Status filter on top.
  const showStatusFilter = !baseFilter?.statuses?.length;

  // Follow the default score setting until the user picks another view here.
  const touchedKind = useRef(false);
  useEffect(() => {
    if (!touchedKind.current && settings?.defaultScore) setScoreKind(settings.defaultScore);
  }, [settings?.defaultScore]);

  const applySearch = useMemo(() => debounce((v: string) => setSearch(v), 180), []);
  useEffect(() => () => applySearch.cancel(), [applySearch]);

  const facets = useResource(() => client.facets(), [], { events: ['library.changed', 'scan.finished', 'scan.item'], throttleMs: 3000 });

  // The score shown only matters to the query when a score range applies to it,
  // so switching scores otherwise keeps the rows and the selection.
  const rangeKind = range ? scoreKind : null;
  const filter = useMemo<LibraryFilter>(() => {
    const r = SCORE_RANGES.find((x) => x.id === range);
    const f: LibraryFilter = { ...baseFilter };
    if (search) f.search = search;
    if (formats.length) f.formats = formats;
    if (flags.length) f.flags = flags;
    if (statuses.length && showStatusFilter) f.statuses = statuses;
    if (r && rangeKind) {
      f.scoreKind = rangeKind;
      if (r.min != null) f.minScore = r.min;
      if (r.max != null) f.maxScore = r.max;
    }
    if (bpmActive(bpm)) {
      if (bpm.min != null) f.bpmMin = bpm.min;
      if (bpm.max != null) f.bpmMax = bpm.max;
      if (bpm.halfDouble) f.bpmHalfDouble = true;
    }
    const keys = keysFor(keySel);
    if (keys.length) f.keys = keys;
    if (genres.length) f.genres = genres;
    return f;
  }, [baseFilter, rangeKind, search, formats, flags, statuses, range, showStatusFilter, bpm, keySel, genres]);
  const filterKey = JSON.stringify(filter);

  const filtersActive =
    !!search || formats.length > 0 || flags.length > 0 || statuses.length > 0 || range != null || bpmActive(bpm) || keySel.keys.length > 0 || genres.length > 0;
  const clearFilters = () => {
    setSearchInput('');
    setSearch('');
    setFormats([]);
    setFlags([]);
    setStatuses([]);
    setRange(null);
    setBpm(NO_BPM);
    setKeySel(NO_KEYS);
    setGenres([]);
  };

  // Drop selected ids that left the result set only on explicit actions; keep it simple and predictable.
  const clearSelection = useCallback(() => setSelection(new Set()), []);
  useEffect(() => {
    clearSelection();
  }, [filterKey, clearSelection]);

  /** Every removal path (Delete key, selection bar, details panel) ends here. */
  const removeTracks = useCallback(
    async (ids: number[], what?: string) => {
      const ok = await actions.remove(ids, what);
      if (!ok) return;
      setSelection(new Set());
      tableRef.current?.removed(ids);
      // The button that started this may be gone; keep the keyboard in the table.
      tableRef.current?.focus();
    },
    [actions],
  );

  // ⌘/Ctrl+F focuses search.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const fc = facets.data;
  const formatItems: MenuItem[] = [
    { kind: 'check', id: 'lossless', label: 'Any lossless', checked: formats.includes('lossless'), count: fc?.lossless, onToggle: () => setFormats(toggle(formats, 'lossless')) },
    { kind: 'check', id: 'lossy', label: 'Any lossy', checked: formats.includes('lossy'), count: fc?.lossy, onToggle: () => setFormats(toggle(formats, 'lossy')) },
    { kind: 'separator', id: 'sep' },
    ...(fc?.formats ?? []).map(
      (f): MenuItem => ({ kind: 'check', id: f.id, label: f.id, checked: formats.includes(f.id), count: f.count, onToggle: () => setFormats(toggle(formats, f.id)) }),
    ),
  ];
  const flagItems: MenuItem[] = (fc?.flags ?? []).map(
    (f): MenuItem => ({ kind: 'check', id: f.id, label: flagInfo(f.id).label, checked: flags.includes(f.id), count: f.count, onToggle: () => setFlags(toggle(flags, f.id)) }),
  );
  const statusCount = (id: string) => (id === 'missing' ? fc?.missing : fc?.statuses.find((s) => s.id === id)?.count);
  const statusItems: MenuItem[] = STATUSES.map(
    (s): MenuItem => ({ kind: 'check', id: s.id, label: s.label, checked: statuses.includes(s.id), count: statusCount(s.id) ?? 0, onToggle: () => setStatuses(toggle(statuses, s.id)) }),
  );
  const rangeItems: MenuItem[] = SCORE_RANGES.map(
    (r): MenuItem => ({ kind: 'check', id: r.id, label: r.label, checked: range === r.id, onToggle: () => setRange(range === r.id ? null : r.id) }),
  );
  const genreItems: MenuItem[] = (fc?.genres ?? []).map(
    (g): MenuItem => ({ kind: 'check', id: `g-${g.id}`, label: g.id, checked: genres.includes(g.id), count: g.count, onToggle: () => setGenres(toggle(genres, g.id)) }),
  );
  const keyCounts = useMemo(() => new Map((fc?.keys ?? []).map((k) => [k.key, k.count])), [fc?.keys]);
  const notation = settings?.keyNotation ?? 'camelot';
  const djPending = offerDjDetection ? (fc?.djPending ?? 0) : 0;

  const detectDj = async () => {
    try {
      toast(describeQueued(await client.analyzeDj()));
      facets.reload();
    } catch (e) {
      toast({ tone: 'error', title: 'Could not queue the detection', body: e instanceof Error ? e.message : String(e) });
    }
  };

  /** The selection, or every track the filters show. */
  const exportForDj = async () => {
    if (selection.size > 0) {
      actions.exportDj([...selection]);
      return;
    }
    try {
      const r = await client.ids(filter);
      actions.exportDj(r.ids);
    } catch (e) {
      toast({ tone: 'error', title: 'Could not list the tracks', body: e instanceof Error ? e.message : String(e) });
    }
  };

  /** A name for a smart playlist made of the filters shown: "House 120–125 BPM". */
  const suggestName = () => {
    const parts: string[] = [];
    if (genres.length) parts.push(genres.slice(0, 2).join(' + '));
    if (bpmActive(bpm)) parts.push(`${bpm.min != null ? formatBpm(bpm.min) : '…'}–${bpm.max != null ? formatBpm(bpm.max) : '…'} BPM`);
    const keys = keySel.keys.map((k) => formatKey(k, notation));
    if (keys.length) parts.push(keys.length <= 3 ? keys.join(' ') : `${keys.length} keys`);
    if (search) parts.push(`“${search}”`);
    return parts.join(' ') || 'Smart playlist';
  };

  const filterButton = (text: string, n: number) => (p: Parameters<Parameters<typeof Menu>[0]['trigger']>[0]) => (
    <Button size="sm" variant={n > 0 ? 'secondary' : 'ghost'} className={n > 0 ? 'is-filtered' : ''} trailing={<LuChevronDown className="btn-chevron" aria-hidden />} {...p}>
      {text}
      {n > 0 ? <span className="btn-count num">{n}</span> : null}
    </Button>
  );

  const selectedIds = useMemo(() => [...selection], [selection]);
  // The panel appears once there is something to show, so an empty panel never takes space.
  const showInspector = inspectorOpen && (active != null || selection.size > 1);
  const exportTarget = selection.size > 0 ? { ids: selectedIds } : { filter };

  return (
    // The toolbar spans the full width and never changes height when rows
    // are selected or the details panel opens, so rows never move under the pointer.
    <div className="browser" {...(panel ? { role: 'tabpanel', id: panel.id, 'aria-labelledby': panel.labelledBy } : {})}>
      {djPending > 0 ? (
        <div className="banner banner-info" role="status">
          <LuAudioLines aria-hidden />
          <span>
            {plural(djPending, 'track')} {djPending === 1 ? 'was' : 'were'} analyzed before SONORA detected tempo and key. Detecting them analyzes{' '}
            {djPending === 1 ? 'that file' : 'those files'} again in the background.
          </span>
          <Button size="sm" variant="primary" onClick={() => void detectDj()}>
            Detect BPM and key
          </Button>
        </div>
      ) : null}
      <div className="toolbar" role="toolbar" aria-label="Filters">
        <div className="toolbar-row">
          <SearchField
            ref={searchRef}
            value={searchInput}
            onChange={(v) => {
              setSearchInput(v);
              applySearch(v);
            }}
            placeholder="Search title, artist, album, file"
            label="Search tracks"
            shortcut={bridge.platform === 'macos' ? '⌘F' : 'Ctrl+F'}
            width={260}
          />
          <Segmented
            label="Score shown"
            size="sm"
            value={scoreKind}
            onChange={(k) => {
              touchedKind.current = true;
              setScoreKind(k);
            }}
            options={SCORE_KINDS.map((k) => ({ value: k, label: SCORE_LABEL[k], hint: SCORE_HINT[k] }))}
          />
          <div className="toolbar-end">
            <span className="toolbar-count faint" aria-live="polite">
              {total == null ? '' : selection.size > 0 ? `${formatCount(selection.size)} of ${plural(total, 'track')}` : plural(total, 'track')}
            </span>
            <IconButton
              label={selection.size > 0 ? `Export ${plural(selection.size, 'selected track')} for DJ software` : 'Export these tracks for DJ software'}
              icon={LuUsb}
              onClick={() => void exportForDj()}
              disabled={!total}
            />
            <IconButton
              label={selection.size > 0 ? `Export ${plural(selection.size, 'selected track')} as CSV` : 'Export these tracks as CSV'}
              icon={LuDownload}
              onClick={() => void actions.exportCsv(exportTarget, selection.size || total || 0)}
              disabled={!total}
            />
            <IconButton
              label={inspectorOpen ? 'Hide details panel' : 'Show details panel'}
              icon={inspectorOpen ? LuPanelRightClose : LuPanelRightOpen}
              pressed={inspectorOpen}
              onClick={() => setInspectorOpen(!inspectorOpen)}
            />
          </div>
        </div>
        <div className="toolbar-filters">
          <Menu label="Score filter" items={rangeItems} trigger={filterButton('Score', range ? 1 : 0)} />
          <Menu label="Format filter" items={formatItems} trigger={filterButton('Format', formats.length)} />
          {showIssueFilter ? (
            <Menu
              label="Issue filter"
              items={flagItems.length ? flagItems : [{ id: 'none', label: 'No issues found yet', disabled: true, onSelect: () => undefined }]}
              trigger={filterButton('Issues', flags.length)}
            />
          ) : null}
          {showStatusFilter ? <Menu label="Status filter" items={statusItems} trigger={filterButton('Status', statuses.length)} /> : null}
          <span className="toolbar-sep" aria-hidden />
          <BpmFilter value={bpm} onChange={setBpm} playingBpm={player.track?.djBpm ?? null} />
          <KeyFilter value={keySel} onChange={setKeySel} counts={keyCounts} notation={notation} playingKey={player.track?.djKey ?? null} />
          <Menu
            label="Genre filter"
            items={genreItems.length ? genreItems : [{ id: 'none', label: 'No genres yet: they come from tags, folders, edits or Discogs', disabled: true, onSelect: () => undefined }]}
            trigger={filterButton('Genre', genres.length)}
          />
          {filtersActive ? (
            <>
              <Button size="sm" variant="ghost" icon={LuX} onClick={clearFilters}>
                Clear
              </Button>
              <Button
                size="sm"
                variant="ghost"
                icon={LuListFilter}
                data-tip="A playlist of the tracks these filters show, that keeps up as the library changes"
                onClick={() => playlists.saveFilters(filter, sort, suggestName())}
              >
                Save as smart playlist
              </Button>
            </>
          ) : null}
        </div>
      </div>
      <div className={`browser-body ${showInspector ? 'with-inspector' : ''}`}>
        <div className="browser-table">
          <TrackTable
            label={label}
            filter={filter}
            sort={sort}
            onSort={setSort}
            scoreKind={scoreKind}
            selection={selection}
            onSelection={setSelection}
            onActive={setActive}
            onOpen={(r) => (r.status === 'done' ? navigate({ view: 'track', id: r.id }) : setActive(r))}
            onDelete={(ids, what) => void removeTracks(ids, what)}
            handle={tableRef}
            onTotal={setTotal}
            empty={
              filtersActive ? (
                <div className="empty-inline">
                  <p>No tracks match these filters.</p>
                  <Button size="sm" onClick={clearFilters}>
                    Clear filters
                  </Button>
                </div>
              ) : (
                emptyBase
              )
            }
          />
          {selection.size > 1 && !showInspector ? (
            // Floats over the table instead of pushing it down.
            <div className="selection-bar" role="region" aria-label="Selection actions">
              <span>{plural(selection.size, 'track')} selected</span>
              <AddToPlaylist ids={selectedIds} size="sm" />
              <Button size="sm" icon={LuPencil} onClick={() => actions.editDj({ ids: selectedIds })}>
                Edit…
              </Button>
              <Button size="sm" icon={LuUsb} onClick={() => actions.exportDj(selectedIds)}>
                Export for DJs…
              </Button>
              <Button size="sm" icon={LuRefreshCw} onClick={() => void actions.reanalyze(selectedIds)}>
                Re-analyze
              </Button>
              <Button size="sm" icon={LuTrash2} variant="ghost" className="is-danger-text" onClick={() => void removeTracks(selectedIds)}>
                Remove from library
              </Button>
              <Button size="sm" variant="ghost" onClick={clearSelection}>
                Clear selection
              </Button>
            </div>
          ) : null}
        </div>
        {showInspector ? (
          <Inspector row={active} selectionCount={selection.size} selectedIds={selectedIds} actions={actions} onRemove={removeTracks} onClearSelection={clearSelection} />
        ) : null}
      </div>
      {actions.dialogs}
    </div>
  );
}
