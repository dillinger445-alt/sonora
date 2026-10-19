//! Duplicate detection and collection hygiene.
//!
//! Three kinds of relationship, from strongest to weakest:
//! - **exact**: byte-identical files (full-file hash);
//! - **same_audio**: identical decoded audio in different files (e.g. the
//!   same master as FLAC and WAV, or re-tagged copies) — decoded-audio hash;
//! - **same_recording** / **alternate**: acoustic-fingerprint matches —
//!   the same recording in a different encoding or resolution, or an
//!   alternate version (edit, extended mix) sharing long stretches of audio.
//!
//! Titles and file names are never used to decide that tracks match.
//! Nothing is ever deleted: the interface only offers removing index entries
//! and revealing files so the user can decide.

use crate::db::sqlite::Connection;
use crate::db::{now_ms, Db, StoreError};
use crate::library::get_tracks;
use crate::model::TrackRow;
use crate::params;
use sonora_engine::analysis::fingerprint::{best_near, from_bytes, Comparison};
use sonora_engine::analysis::spectral::FP_RATE;
use sonora_json::{num, obj, ToJson, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Indexed sub-fingerprints: every Nth frame of each track, denser for short
/// tracks so they still collect enough votes (at least ~64 positions).
const MAX_INDEX_STEP: usize = 8;
/// Bits of each sub-fingerprint used as the lookup key.
const KEY_BITS: u32 = 18;
/// Postings lists longer than this are too common to be informative.
const MAX_POSTINGS: usize = 4000;
/// Minimum consistent-offset votes before a pair is verified.
const MIN_VOTES: u16 = 4;

fn key_of(v: u32) -> u32 {
    (v >> 7) & ((1 << KEY_BITS) - 1)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub a: i64,
    pub b: i64,
    pub kind: &'static str,
    pub cmp: Comparison,
}

/// Classifies a verified comparison between tracks of the given durations.
pub fn classify(cmp: &Comparison, dur_a: f64, dur_b: f64) -> Option<&'static str> {
    let shorter = dur_a.min(dur_b).max(1.0);
    let longest = cmp.longest_run_secs();
    let close_length = (dur_a - dur_b).abs() <= (3.0f64).max(0.05 * dur_a.max(dur_b));
    // Whole-track bit error rate measured on real variants: 0.00–0.13 for
    // re-encodes (MP3, AAC, Opus, resampled, mildly clipped), ~0.22 for an
    // extremely clipped copy; unrelated music 0.30–0.50, even when it uses
    // the same sounds and arrangement.
    let aligned = cmp.overlap as f64 / FP_RATE >= 0.8 * shorter;
    if cmp.ber <= 0.22 && aligned && close_length {
        Some("same_recording")
    } else if longest >= 20.0f64.min(0.5 * shorter) && cmp.matched_secs() >= 15.0 {
        Some("alternate")
    } else {
        None
    }
}

/// Finds fingerprint matches across the whole library.
pub fn compute_matches(db: &Db, cancel: &AtomicBool, mut progress: impl FnMut(f64)) -> Result<Vec<Match>, StoreError> {
    // Load fingerprints (compact) for analysed, present tracks.
    let tracks: Vec<(i64, f64, Vec<u32>)> = {
        let c = db.read();
        c.query(
            "SELECT t.id, t.duration, a.fingerprint FROM tracks t JOIN analyses a ON a.track_id = t.id \
             WHERE t.status = 'done' AND t.missing_since IS NULL AND a.fingerprint IS NOT NULL",
            params![],
            |r| (r.i64(0), r.opt_f64(1).unwrap_or(0.0), from_bytes(&r.opt_blob(2).unwrap_or_default())),
        )?
    };
    let mut index: HashMap<u32, Vec<(u32, u32)>> = HashMap::new();
    for (ti, (_, _, fp)) in tracks.iter().enumerate() {
        let step = (fp.len() / 64).clamp(1, MAX_INDEX_STEP);
        for pos in (0..fp.len()).step_by(step) {
            let v = fp[pos];
            if v != 0 {
                index.entry(key_of(v)).or_default().push((ti as u32, pos as u32));
            }
        }
    }
    let mut matches = Vec::new();
    let mut votes: HashMap<(u32, i64), u16> = HashMap::new();
    for (ai, (a_id, a_dur, a_fp)) in tracks.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        votes.clear();
        for (pa, &v) in a_fp.iter().enumerate() {
            if v == 0 {
                continue;
            }
            let Some(post) = index.get(&key_of(v)) else { continue };
            if post.len() > MAX_POSTINGS {
                continue;
            }
            for &(bi, pb) in post {
                if bi as usize <= ai {
                    continue;
                }
                *votes.entry((bi, pa as i64 - pb as i64)).or_insert(0) += 1;
            }
        }
        // Best offset per candidate track.
        let mut best: HashMap<u32, (i64, u16)> = HashMap::new();
        for (&(bi, off), &n) in &votes {
            if n >= MIN_VOTES {
                let e = best.entry(bi).or_insert((off, 0));
                if n > e.1 {
                    *e = (off, n);
                }
            }
        }
        for (bi, (off, _)) in best {
            let (b_id, b_dur, b_fp) = &tracks[bi as usize];
            // votes count positions in A against B; b[i] ↔ a[i + off].
            if let Some(cmp) = best_near(a_fp, b_fp, off, 2) {
                if let Some(kind) = classify(&cmp, *a_dur, *b_dur) {
                    matches.push(Match { a: *a_id, b: *b_id, kind, cmp });
                }
            }
        }
        progress((ai + 1) as f64 / tracks.len().max(1) as f64);
    }
    Ok(matches)
}

/// Replaces the stored fingerprint matches.
pub fn store_matches(c: &Connection, matches: &[Match]) -> Result<(), StoreError> {
    c.transaction(|tx| {
        tx.execute("DELETE FROM fp_matches", params![])?;
        for m in matches {
            let (a, b) = if m.a < m.b { (m.a, m.b) } else { (m.b, m.a) };
            tx.execute(
                "INSERT OR REPLACE INTO fp_matches (a, b, kind, ber, offset_secs, matched_secs) VALUES (?, ?, ?, ?, ?, ?)",
                params![a, b, m.kind, m.cmp.ber, m.cmp.offset_secs(), m.cmp.matched_secs()],
            )?;
        }
        tx.execute("INSERT OR REPLACE INTO meta (key, value) VALUES ('duplicates_scanned_at', ?)", params![now_ms().to_string()])?;
        Ok::<(), StoreError>(())
    })
}

struct UnionFind {
    parent: HashMap<i64, i64>,
}

impl UnionFind {
    fn find(&mut self, x: i64) -> i64 {
        let p = *self.parent.entry(x).or_insert(x);
        if p == x {
            return x;
        }
        let r = self.find(p);
        self.parent.insert(x, r);
        r
    }
    fn union(&mut self, a: i64, b: i64) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.parent.insert(ra.max(rb), ra.min(rb));
        }
    }
}

/// Ranks versions: better score, lossless, higher resolution, larger file.
fn keep_rank(t: &TrackRow) -> (i64, i64, i64, i64, i64) {
    (
        t.overall.map(|v| (v * 10.0) as i64).unwrap_or(-1),
        t.lossless.map(|l| l as i64).unwrap_or(0),
        t.bit_depth.unwrap_or(0) * 1_000_000 + t.sample_rate.unwrap_or(0),
        t.bitrate_kbps.map(|v| v as i64).unwrap_or(0),
        t.size,
    )
}

fn group_json(kind: &str, key: String, members: Vec<TrackRow>, details: HashMap<i64, Value>, similarity: f64) -> Value {
    let keep = members.iter().filter(|t| t.missing_since.is_none()).max_by_key(|t| keep_rank(t)).map(|t| t.id);
    let label = match kind {
        "exact" => "Identical files",
        "same_audio" => "Identical audio, different files",
        "same_recording" => "Same recording, different encoding",
        _ => "Possible alternate versions",
    };
    let explanation = match kind {
        "exact" => "Byte-for-byte identical files (same full-file hash).",
        "same_audio" => "The decoded audio is sample-for-sample identical; only the container, tags or metadata differ.",
        "same_recording" => "Acoustic fingerprints match along the whole track, but the audio is not bit-identical — typically the same master in another format, bitrate or resolution.",
        _ => "Long stretches of identical audio with different lengths or arrangements — for example a radio edit and an extended mix. Review before removing anything.",
    };
    obj! {
        "id" => key,
        "kind" => kind,
        "label" => label,
        "explanation" => explanation,
        "similarity" => num(similarity, 3),
        "suggestedKeep" => keep,
        "members" => Value::Array(members.iter().map(|t| {
            let mut v = t.to_json();
            if let Some(d) = details.get(&t.id) {
                v.set("match", d.clone());
            }
            v
        }).collect()),
    }
}

/// All duplicate groups, strongest relationship first.
pub fn list(c: &Connection) -> Result<Value, StoreError> {
    let mut groups: Vec<Value> = Vec::new();
    let mut grouped_pairs: std::collections::HashSet<(i64, i64)> = std::collections::HashSet::new();

    let exact = c.query(
        "SELECT file_hash, id FROM tracks WHERE file_hash IN (SELECT file_hash FROM tracks WHERE file_hash IS NOT NULL GROUP BY file_hash HAVING COUNT(*) > 1) ORDER BY file_hash, id",
        params![],
        |r| (r.text(0), r.i64(1)),
    )?;
    let mut by_hash: Vec<(String, Vec<i64>)> = Vec::new();
    for (h, id) in exact {
        match by_hash.last_mut() {
            Some((lh, ids)) if *lh == h => ids.push(id),
            _ => by_hash.push((h, vec![id])),
        }
    }
    for (h, ids) in by_hash {
        for i in 0..ids.len() {
            for j in i + 1..ids.len() {
                grouped_pairs.insert((ids[i], ids[j]));
            }
        }
        groups.push(group_json("exact", format!("exact:{h}"), get_tracks(c, &ids)?, HashMap::new(), 1.0));
    }

    let same = c.query(
        "SELECT audio_hash, id FROM tracks WHERE audio_hash IN (SELECT audio_hash FROM tracks WHERE audio_hash IS NOT NULL GROUP BY audio_hash \
         HAVING COUNT(DISTINCT COALESCE(file_hash, CAST(id AS TEXT))) > 1) ORDER BY audio_hash, id",
        params![],
        |r| (r.text(0), r.i64(1)),
    )?;
    let mut by_audio: Vec<(String, Vec<i64>)> = Vec::new();
    for (h, id) in same {
        match by_audio.last_mut() {
            Some((lh, ids)) if *lh == h => ids.push(id),
            _ => by_audio.push((h, vec![id])),
        }
    }
    for (h, ids) in by_audio {
        for i in 0..ids.len() {
            for j in i + 1..ids.len() {
                grouped_pairs.insert((ids[i], ids[j]));
            }
        }
        groups.push(group_json("same_audio", format!("audio:{h}"), get_tracks(c, &ids)?, HashMap::new(), 1.0));
    }

    for kind in ["same_recording", "alternate"] {
        let pairs = c.query("SELECT a, b, ber, offset_secs, matched_secs FROM fp_matches WHERE kind = ? ORDER BY a, b", params![kind], |r| {
            (r.i64(0), r.i64(1), r.f64(2), r.f64(3), r.f64(4))
        })?;
        let pairs: Vec<_> = pairs.into_iter().filter(|(a, b, ..)| !grouped_pairs.contains(&(*a.min(b), *a.max(b)))).collect();
        let mut uf = UnionFind { parent: HashMap::new() };
        for (a, b, ..) in &pairs {
            uf.union(*a, *b);
        }
        let mut comps: HashMap<i64, Vec<i64>> = HashMap::new();
        let ids: Vec<i64> = pairs.iter().flat_map(|(a, b, ..)| [*a, *b]).collect();
        for id in ids {
            let root = uf.find(id);
            let e = comps.entry(root).or_default();
            if !e.contains(&id) {
                e.push(id);
            }
        }
        let mut roots: Vec<i64> = comps.keys().copied().collect();
        roots.sort_unstable();
        for root in roots {
            let members = &comps[&root];
            let rel: Vec<_> = pairs.iter().filter(|(a, b, ..)| members.contains(a) && members.contains(b)).collect();
            let avg_ber = rel.iter().map(|p| p.2).sum::<f64>() / rel.len().max(1) as f64;
            let mut details = HashMap::new();
            for (a, b, ber, off, matched) in &rel {
                details.insert(*b, obj! { "with" => *a, "ber" => num(*ber, 3), "offsetSecs" => num(*off, 1), "matchedSecs" => num(*matched, 0) });
            }
            let mut sorted = members.clone();
            sorted.sort_unstable();
            groups.push(group_json(kind, format!("{kind}:{root}"), get_tracks(c, &sorted)?, details, (1.0 - 2.0 * avg_ber).clamp(0.0, 1.0)));
        }
    }
    let scanned_at = c.query_row("SELECT value FROM meta WHERE key = 'duplicates_scanned_at'", params![], |r| r.text(0))?.and_then(|v| v.parse::<i64>().ok());
    let analyzed_since = match scanned_at {
        Some(t) => c.query_row("SELECT COUNT(*) FROM tracks WHERE status = 'done' AND analyzed_at > ?", params![t], |r| r.i64(0))?.unwrap_or(0),
        None => c.query_row("SELECT COUNT(*) FROM tracks WHERE status = 'done'", params![], |r| r.i64(0))?.unwrap_or(0),
    };
    Ok(obj! {
        "groups" => Value::Array(groups),
        "fingerprintScannedAt" => scanned_at,
        "analyzedSinceScan" => analyzed_since,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification_rules() {
        let full = Comparison { offset: 0, ber: 0.12, overlap: 3000, matched: 2950, longest_run: 2900 };
        assert_eq!(classify(&full, 300.0, 301.0), Some("same_recording"));
        // Same audio stretch but very different lengths: an edit.
        let edit = Comparison { offset: 120, ber: 0.3, overlap: 1800, matched: 900, longest_run: 900 };
        assert_eq!(classify(&edit, 400.0, 180.0), Some("alternate"));
        let none = Comparison { offset: 0, ber: 0.49, overlap: 3000, matched: 0, longest_run: 0 };
        assert_eq!(classify(&none, 300.0, 300.0), None);
    }
}
