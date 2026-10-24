/**
 * Sorting tracks into playlists: one per genre, key or tempo band, in a new
 * folder. For the whole library they can be smart playlists that keep up
 * with new tracks; for chosen tracks they are fixed playlists.
 */

import { useEffect, useState } from 'react';
import type { OrganizeBy } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { Segmented } from '../components/Controls.tsx';
import { Dialog } from '../components/Dialog.tsx';
import { plural } from '../lib/format.ts';
import { formatKey } from '../lib/keys.ts';
import { navigate } from '../lib/router.ts';
import { organizeLeftOut } from '../lib/rules.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';

export interface OrganizeTarget {
  /** Chosen tracks, or `null` for the whole library. */
  ids: number[] | null;
}

const BY: { value: OrganizeBy; label: string }[] = [
  { value: 'genre', label: 'Genre' },
  { value: 'key', label: 'Key' },
  { value: 'tempo', label: 'Tempo' },
];

export function OrganizeDialog({ target, onClose }: { target: OrganizeTarget | null; onClose: () => void }) {
  const { client, settings } = useCore();
  const toast = useToast();
  const [by, setBy] = useState<OrganizeBy>('genre');
  const [smart, setSmart] = useState(true);
  const [busy, setBusy] = useState(false);
  const open = target != null;
  const whole = target?.ids == null;
  const notation = settings?.keyNotation ?? 'camelot';

  useEffect(() => {
    if (open) setBusy(false);
  }, [open]);

  const example =
    by === 'genre'
      ? 'House, Techno, Disco…'
      : by === 'key'
        ? `${formatKey(21, notation)}, ${formatKey(16, notation)}, ${formatKey(0, notation)}…`
        : '120–124 BPM, 125–129 BPM…';

  const run = async () => {
    setBusy(true);
    try {
      const r = await client.organize(by, { smart: whole && smart, trackIds: target?.ids ?? undefined, keyNotation: notation });
      const left = organizeLeftOut(r, by);
      toast({ tone: 'success', title: `Made ${plural(r.playlists, 'playlist')} of ${plural(r.tracks, 'track')}`, body: left });
      onClose();
      navigate({ view: 'playlist', id: r.folderId });
    } catch (e) {
      toast({ tone: 'error', title: 'Could not sort into playlists', body: e instanceof Error ? e.message : String(e) });
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      title={whole ? 'Sort the library into playlists' : `Sort ${plural(target?.ids?.length ?? 0, 'track')} into playlists`}
      onClose={onClose}
      width={500}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" busy={busy} onClick={() => void run()} data-autofocus>
            Make playlists
          </Button>
        </>
      }
    >
      <div className="organize">
        <p className="dialog-text">A new folder gets one playlist per {by === 'tempo' ? 'tempo band of 5 BPM' : by}, sorted by tempo inside. Nothing else changes.</p>
        <div className="organize-row">
          <span className="xp-label">Sort by</span>
          <Segmented label="Sort by" value={by} onChange={setBy} options={BY} />
        </div>
        <p className="organize-example faint">{example}</p>
        {whole ? (
          <label className="check-row">
            <input type="checkbox" className="check" checked={smart} onChange={(e) => setSmart(e.target.checked)} />
            <span>
              Keep them up to date <span className="faint">(smart playlists: new and changed tracks move in by themselves)</span>
            </span>
          </label>
        ) : null}
      </div>
    </Dialog>
  );
}
