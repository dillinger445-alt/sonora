/** Tempo, key and genre on the track page: the values in use, how sure the detection is, and quick corrections. */

import type { Measurements, TrackRow } from '../api/types.ts';
import { Button } from '../components/Button.tsx';
import { BpmText, KeyBadge, SourceNote } from '../components/DjMarks.tsx';
import { DASH, formatPercent } from '../lib/format.ts';
import { formatBpm, formatKey, longKeyName } from '../lib/keys.ts';
import { useCore } from '../state/core.tsx';
import { useToast } from '../state/toasts.tsx';
import { DiscogsLink } from './Inspector.tsx';

function confidenceWord(c: number | null | undefined): string {
  if (c == null) return '';
  if (c >= 0.6) return 'high confidence';
  if (c >= 0.3) return 'medium confidence';
  return 'low confidence';
}

export function TrackDj({ t, m, onEdit }: { t: TrackRow; m: Measurements | null; onEdit: () => void }) {
  const { client, settings } = useCore();
  const toast = useToast();
  const notation = settings?.keyNotation ?? 'camelot';
  const tempo = m?.tempo ?? null;
  const key = m?.key ?? null;
  // The other readings a DJ might count: half, double, or a triplet feel (×2/3, ×3/2).
  const ref = t.djBpm ?? tempo?.bpm ?? null;
  const others =
    ref == null
      ? []
      : (tempo?.candidates ?? [])
          .filter(([b]) => [0.5, 2, 2 / 3, 1.5].some((r) => Math.abs(b / ref - r) < 0.03))
          .slice(0, 3)
          .map(([b]) => formatBpm(b));

  const scale = async (f: 0.5 | 2) => {
    try {
      await client.editDj([t.id], { bpmFactor: f });
    } catch (e) {
      toast({ tone: 'error', title: 'Could not change the tempo', body: e instanceof Error ? e.message : String(e) });
    }
  };

  const tuningHz = key ? 440 * 2 ** (key.tuningCents / 1200) : null;

  return (
    <div className="track-dj">
      <section className="track-dj-cell" aria-label="Tempo">
        <h3 className="track-dj-label">
          Tempo <SourceNote source={t.djBpmSource} />
        </h3>
        <div className="track-dj-value">
          {t.djBpm != null ? (
            <>
              <BpmText bpm={t.djBpm} variable={t.tempoVariable} /> <span className="faint">BPM</span>
            </>
          ) : (
            <span className="faint">Not known</span>
          )}
        </div>
        <ul className="track-dj-facts">
          {tempo ? (
            <li>
              Detected {formatBpm(tempo.bpm)} BPM, {confidenceWord(tempo.confidence)} <span className="num faint">({tempo.confidence.toFixed(2)})</span>
            </li>
          ) : m ? (
            <li className="faint">{m.tempo === undefined ? 'Analyzed before tempo detection existed; re-analyze to detect it.' : 'No steady pulse was found.'}</li>
          ) : null}
          {tempo?.variable ? <li className="t-amber">The tempo changes or drifts during the track.</li> : null}
          {others.length ? <li className="faint">Other readings: {others.join(', ')} BPM</li> : null}
        </ul>
        {t.djBpm != null ? (
          <div className="track-dj-actions">
            <Button size="sm" onClick={() => void scale(2)} data-tip="It reads at half time: double it">
              ×2
            </Button>
            <Button size="sm" onClick={() => void scale(0.5)} data-tip="It reads at double time: halve it">
              ÷2
            </Button>
          </div>
        ) : null}
      </section>
      <section className="track-dj-cell" aria-label="Key">
        <h3 className="track-dj-label">
          Key <SourceNote source={t.djKeySource} />
        </h3>
        <div className="track-dj-value is-key">
          {t.djKey != null ? (
            <>
              <KeyBadge keyId={t.djKey} notation={notation} size="lg" /> <span className="track-dj-name">{longKeyName(t.djKey)}</span>
            </>
          ) : (
            <span className="faint">Not known</span>
          )}
        </div>
        <ul className="track-dj-facts">
          {key ? (
            <li>
              Detected {formatKey(key.key, notation)}, {confidenceWord(key.confidence)} <span className="num faint">({key.confidence.toFixed(2)})</span>
            </li>
          ) : m ? (
            <li className="faint">{m.key === undefined ? 'Analyzed before key detection existed; re-analyze to detect it.' : 'No clear key: little tonal material, or atonal.'}</li>
          ) : null}
          {key ? (
            <li className="faint">
              Next most likely: {formatKey(key.second, notation)} ({longKeyName(key.second)})
            </li>
          ) : null}
          {key?.changes ? <li className="t-amber">The key seems to change during the track.</li> : null}
          {key && tuningHz != null ? (
            <li className="faint" data-tip="Concert pitch the track is tuned to (A above middle C)">
              Tuned to A = <span className="num">{tuningHz.toFixed(1)}</span> Hz
              {Math.abs(key.tuningCents) >= 1 ? (
                <span className="num">
                  {' '}
                  ({key.tuningCents > 0 ? '+' : '−'}
                  {Math.abs(key.tuningCents).toFixed(0)} cents)
                </span>
              ) : null}
            </li>
          ) : null}
        </ul>
      </section>
      <section className="track-dj-cell" aria-label="Genre">
        <h3 className="track-dj-label">
          Genre <SourceNote source={t.djGenreSource} />
        </h3>
        <div className="track-dj-value track-dj-genre selectable">{t.djGenre ?? <span className="faint">None</span>}</div>
        <ul className="track-dj-facts">
          {t.djGenreSource === 'discogs' && t.discogsUrl ? (
            <li className="faint">
              Data provided by Discogs: <DiscogsLink url={t.discogsUrl} />
            </li>
          ) : null}
          {t.genre && t.djGenreSource !== 'tag' ? <li className="faint">Tag: {t.genre}</li> : null}
          {key && t.keyConfidence != null && t.djKeySource === 'detected' && t.keyConfidence < 0.3 ? (
            <li className="faint">Low key confidence ({formatPercent(t.keyConfidence)}): check by ear before mixing.</li>
          ) : null}
        </ul>
        <div className="track-dj-actions">
          <Button size="sm" onClick={onEdit}>
            Edit…
          </Button>
        </div>
      </section>
      {t.djBpm == null && t.djKey == null && !t.djGenre && !m ? <p className="faint">{DASH}</p> : null}
    </div>
  );
}
