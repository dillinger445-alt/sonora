/**
 * The rules a YouTube playlist download picks Soulseek files by: the full
 * form in Settings → Playlist download rules, and a short bar on the
 * playlist page with the choices made most often. Both edit the same
 * setting (`playlistRules`); the core checks every value.
 */

import { useId } from 'react';
import { LuChevronDown, LuChevronUp } from 'react-icons/lu';
import type { LosslessFormat, PlaylistMode, PlaylistQuality, PlaylistRules } from '../api/types.ts';
import { formatRateKhz } from '../lib/format.ts';
import { formatRoute } from '../lib/router.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import { Checkbox, NumberStepper, Segmented, Switch } from './Controls.tsx';

export const FORMAT_NAMES: { id: LosslessFormat; label: string; hint: string }[] = [
  { id: 'flac', label: 'FLAC', hint: 'The usual lossless format on Soulseek' },
  { id: 'aiff', label: 'AIFF', hint: 'Uncompressed, full tags' },
  { id: 'wav', label: 'WAV', hint: 'Uncompressed' },
  { id: 'alac', label: 'ALAC', hint: 'Apple Lossless; .m4a files are taken and checked on arrival' },
  { id: 'wv', label: 'WavPack', hint: 'Off unless you turn it on' },
  { id: 'ape', label: 'APE', hint: "Monkey's Audio; off unless you turn it on" },
];

export const QUALITY_OPTIONS: { value: PlaylistQuality; label: string; hint: string }[] = [
  { value: 'cd', label: 'Prefer 16/44.1', hint: 'CD quality first; every player reads it' },
  { value: 'highest', label: 'Prefer highest', hint: '24-bit and high sample rates first' },
  { value: 'any', label: 'Any', hint: 'Resolution does not matter' },
];

export const MODE_OPTIONS: { value: PlaylistMode; label: string; hint: string }[] = [
  { value: 'automatic', label: 'Automatic', hint: 'Download the best file the moment a track is found' },
  { value: 'review', label: 'Review first', hint: 'Search everything, show the picks, download when you confirm' },
];

/** Reads and writes the rules in the settings. */
export function usePlaylistRules(): { rules: PlaylistRules | null; set: (patch: Partial<PlaylistRules>) => void } {
  const { settings, saveSettings } = useCore();
  const toast = useToast();
  const rules = settings?.playlistRules ?? null;
  const set = (patch: Partial<PlaylistRules>) => {
    if (!rules) return;
    void saveSettings({ playlistRules: { ...rules, ...patch } }).catch((e: unknown) => toast({ tone: 'error', title: 'Could not save the rule', body: e instanceof Error ? e.message : String(e) }));
  };
  return { rules, set };
}

/** The choices made most often, on the playlist page. `note`: when they take effect, if not at once. */
export function QuickRules({ note }: { note?: string }) {
  const { rules, set } = usePlaylistRules();
  if (!rules) return null;
  return (
    <div className="pl-quick" role="group" aria-label="Download rules">
      <Segmented
        label="Formats"
        size="sm"
        value={rules.lossy ? 'any' : 'lossless'}
        onChange={(v) => set({ lossy: v === 'any' })}
        options={[
          { value: 'lossless', label: 'Lossless only', hint: rules.formats.map((f) => f.toUpperCase()).join(', ') || 'No lossless format chosen' },
          { value: 'any', label: `Lossy too (${rules.minBitrate}+ kbps)`, hint: 'MP3, AAC, Ogg and Opus after every lossless format' },
        ]}
      />
      <Segmented label="Quality" size="sm" value={rules.quality} onChange={(v) => set({ quality: v })} options={QUALITY_OPTIONS} />
      <Segmented label="Mode" size="sm" value={rules.mode} onChange={(v) => set({ mode: v })} options={MODE_OPTIONS} />
      <a className="link pl-quick-all" href={formatRoute({ view: 'settings' })}>
        All rules
      </a>
      {note ? <span className="pl-quick-note faint">{note}</span> : null}
    </div>
  );
}

/** Every rule, for Settings. */
export function PlaylistRulesForm() {
  const { rules, set } = usePlaylistRules();
  const id = useId();
  if (!rules) return null;
  const move = (f: LosslessFormat, dir: -1 | 1) => {
    const i = rules.formats.indexOf(f);
    const j = i + dir;
    if (i < 0 || j < 0 || j >= rules.formats.length) return;
    const next = [...rules.formats];
    next.splice(i, 1);
    next.splice(j, 0, f);
    set({ formats: next });
  };
  const toggle = (f: LosslessFormat, on: boolean) => {
    const next = on ? [...rules.formats.filter((x) => x !== f), f] : rules.formats.filter((x) => x !== f);
    if (next.length === 0 && !rules.lossy) return;
    set({ formats: next });
  };
  return (
    <div className="pl-rules">
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label" id={`${id}-formats`}>
            Formats, best first
          </span>
          <span className="switch-desc">The first format wins when a track is shared in several. ALAC in .m4a files cannot be told from AAC by name: taken, and checked on arrival.</span>
        </div>
        <ol className="pl-formats" aria-labelledby={`${id}-formats`}>
          {[...rules.formats, ...FORMAT_NAMES.map((f) => f.id).filter((f) => !rules.formats.includes(f))].map((f) => {
            const on = rules.formats.includes(f);
            const meta = FORMAT_NAMES.find((x) => x.id === f);
            const i = rules.formats.indexOf(f);
            return (
              <li key={f} className={on ? 'is-on' : 'faint'}>
                <label className="check-row">
                  <Checkbox checked={on} onChange={(e) => toggle(f, e.target.checked)} />
                  <span>
                    {meta?.label ?? f.toUpperCase()} <span className="faint">{meta?.hint}</span>
                  </span>
                </label>
                {on ? (
                  <span className="pl-format-order">
                    <button type="button" className="stepper-btn" aria-label={`Move ${meta?.label} up`} disabled={i === 0} onClick={() => move(f, -1)}>
                      <LuChevronUp aria-hidden />
                    </button>
                    <button type="button" className="stepper-btn" aria-label={`Move ${meta?.label} down`} disabled={i === rules.formats.length - 1} onClick={() => move(f, 1)}>
                      <LuChevronDown aria-hidden />
                    </button>
                  </span>
                ) : null}
              </li>
            );
          })}
        </ol>
      </div>
      <Switch label="Lossy files too" description="MP3, AAC, Ogg and Opus, after every lossless format, when nothing lossless is shared." checked={rules.lossy} onChange={(v) => set({ lossy: v })} />
      {rules.lossy ? (
        <div className="setting-row">
          <div className="switch-text">
            <span className="switch-label">Lowest bit rate</span>
            <span className="switch-desc">Lossy files below this are left out.</span>
          </div>
          <div className="setting-control">
            <NumberStepper value={rules.minBitrate} min={64} max={320} step={32} unit="kbps" label="Lowest bit rate" onChange={(v) => set({ minBitrate: v })} />
          </div>
        </div>
      ) : null}
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Resolution</span>
          <span className="switch-desc">Which bit depth and sample rate come first among files of the same format.</span>
        </div>
        <div className="setting-control">
          <Segmented label="Resolution preference" size="sm" value={rules.quality} onChange={(v) => set({ quality: v })} options={QUALITY_OPTIONS} />
        </div>
      </div>
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Highest resolution taken</span>
          {/* With their units: "24-bit, 96 kHz", not "24, 96000" (B57). */}
          <span className="switch-desc">
            Files above this are left out: {rules.maxBitDepth == null ? 'no bit depth limit' : `${rules.maxBitDepth}-bit`},{' '}
            {rules.maxSampleRate == null ? 'no sample rate limit' : `${formatRateKhz(rules.maxSampleRate)} kHz`}.
          </span>
        </div>
        <div className="setting-control pl-limits">
          <Segmented
            label="Bit depth limit"
            size="sm"
            value={rules.maxBitDepth == null ? 'none' : String(rules.maxBitDepth)}
            onChange={(v) => set({ maxBitDepth: v === 'none' ? null : Number(v) })}
            options={[
              { value: 'none', label: 'Any bits' },
              { value: '16', label: '≤ 16-bit' },
              { value: '24', label: '≤ 24-bit' },
            ]}
          />
          <Segmented
            label="Sample rate limit"
            size="sm"
            value={rules.maxSampleRate == null ? 'none' : String(rules.maxSampleRate)}
            onChange={(v) => set({ maxSampleRate: v === 'none' ? null : Number(v) })}
            options={[
              { value: 'none', label: 'Any rate' },
              { value: '48000', label: '≤ 48 kHz' },
              { value: '96000', label: '≤ 96 kHz' },
            ]}
          />
        </div>
      </div>
      <Switch
        label="Check the size"
        description="Leave out files far smaller than a lossless file of that length should be (a lossy file renamed, or a cut)."
        checked={rules.sizeCheck}
        onChange={(v) => set({ sizeCheck: v })}
      />
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Largest file taken</span>
          <span className="switch-desc">0 for no limit.</span>
        </div>
        <div className="setting-control">
          <NumberStepper value={rules.maxSizeMb ?? 0} min={0} max={5000} step={50} unit="MB" label="Largest file" onChange={(v) => set({ maxSizeMb: v === 0 ? null : v })} />
        </div>
      </div>
      <Switch label="Only people with a free upload slot" description="Otherwise a short queue is fine too." checked={rules.freeSlot} onChange={(v) => set({ freeSlot: v })} />
      {!rules.freeSlot ? (
        <div className="setting-row">
          <div className="switch-text">
            <span className="switch-label">Longest queue</span>
            <span className="switch-desc">Without a free slot, the download waits behind this many others at most.</span>
          </div>
          <div className="setting-control">
            <NumberStepper value={rules.maxQueue} min={0} max={500} step={5} unit="downloads" label="Longest queue" onChange={(v) => set({ maxQueue: v })} />
          </div>
        </div>
      ) : null}
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Slowest connection</span>
          <span className="switch-desc">The upload speed the other person's client reports; 0 for any.</span>
        </div>
        <div className="setting-control">
          <NumberStepper value={rules.minSpeedKbps} min={0} max={100_000} step={100} unit="kB/s" label="Slowest connection" onChange={(v) => set({ minSpeedKbps: v })} />
        </div>
      </div>
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Title words in the file name</span>
          <span className="switch-desc">The share of the title's words the file name must have. 60 % lets a word or two differ; 100 % wants them all.</span>
        </div>
        <div className="setting-control">
          <NumberStepper value={Math.round(rules.minOverlap * 100)} min={30} max={100} step={10} unit="%" label="Title words in the file name" onChange={(v) => set({ minOverlap: v / 100 })} />
        </div>
      </div>
      <Switch label="The artist must be in the file or folder name" description="When the title names one." checked={rules.requireArtist} onChange={(v) => set({ requireArtist: v })} />
      <Switch
        label="Leave out other versions"
        description="Names that say remix, edit, live, radio or instrumental are left out unless the title says so too."
        checked={rules.rejectVariants}
        onChange={(v) => set({ rejectVariants: v })}
      />
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Mode</span>
          <span className="switch-desc">Automatic downloads each track as soon as it is found. Review first searches everything and waits for you.</span>
        </div>
        <div className="setting-control">
          <Segmented label="Mode" size="sm" value={rules.mode} onChange={(v) => set({ mode: v })} options={MODE_OPTIONS} />
        </div>
      </div>
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Downloads at a time</span>
          <span className="switch-desc">Searching goes on meanwhile, one search at a time.</span>
        </div>
        <div className="setting-control">
          <NumberStepper value={rules.maxDownloads} min={1} max={20} label="Downloads at a time" onChange={(v) => set({ maxDownloads: v })} />
        </div>
      </div>
      <div className="setting-row">
        <div className="switch-text">
          <span className="switch-label">Time per search</span>
          <span className="switch-desc">slskd searches for about 15 seconds by itself; a longer wait gathers more answers.</span>
        </div>
        <div className="setting-control">
          <NumberStepper value={rules.searchSeconds} min={15} max={25} unit="s" label="Time per search" onChange={(v) => set({ searchSeconds: v })} />
        </div>
      </div>
      <Switch
        label="Look again later for tracks not found"
        description="An hour after a run ends, and up to three times, while SONORA is open."
        checked={rules.retryNotFound}
        onChange={(v) => set({ retryNotFound: v })}
      />
    </div>
  );
}
