# Bugs found in the 0.3.2 review

A review of every function in SONORA, done before 0.4: nine read-throughs
of the code (core, engine, shells, every view), the full test suites, and
two tours of the running app on the fixtures library (`npm run fixtures`)
with the console and the Rust log watched. Every finding is listed here,
most severe first, as a checklist: what the person sees, how to make it
happen, why it happens, and the fix with the test that now guards it.

Severity: **critical** — wrong or lost files, a job that crashes, a feature
that cannot work; **high** — a wrong result or a stuck state the person
cannot get out of; **medium** — a wrong number or label, stale state, a
control that does nothing in some case; **low** — minor or cosmetic.

Nothing that worked as designed was changed. Every finding below is fixed.
Each says which test guards it; the few that no automated test can reach
say why, and how the fix was checked instead.

## Critical

- [x] **B01. Playlist downloads never arrive in the library.**
  - Symptom: in a YouTube playlist run, every finished download stays on
    "Arrived; checking it". After 10 minutes SONORA gives up on it, downloads
    the next candidate, and after three tries marks the track failed. The
    files are only imported once nothing is waiting, as ordinary Soulseek
    downloads the run never sees.
  - Steps: run a playlist with the normal settings (no
    `SONORA_SOURCE_POLL_SECONDS`) and let a download finish.
  - Cause: the run wakes the folder watcher every 2 s while it waits, and
    `Watcher::wake()` also forgets every file's last size and time. A file
    is taken only when two looks in a row see the same size, so with a wake
    before every look it never is. The tests passed because they set a
    1-second poll, which squeezes a look between two wakes.
  - Fix: a `nudge()` that asks for a look without forgetting anything; the
    run uses it, and `wake()` stays for settings changes.
  - Test: every test in `crates/sonora-core/tests/playlist.rs` now runs with the watcher's normal poll, which wakes it as the app does (they all hung before).

- [x] **B02. A date tag that is not plain ASCII crashes DJ export and Send to USB.**
  - Symptom: "The export stopped because of an internal error" and nothing
    is written, for the whole selection. On a USB update the drive's old
    copy of the track stays renamed to a hidden `.sonora-old-0`, so the
    track disappears from the stick while its playlist still lists it.
  - Steps: tag a FLAC with `DATE=二〇一九` (or `２０２４`, `平成11年`), import
    it and export it in any format.
  - Cause: the year is cut with `y[..4]`, which counts bytes; when byte 4 is
    inside a character Rust panics. `id3::build`, `riff_info` and
    `djexport` all did it.
  - Fix: one `year_of()` that takes the first four characters and accepts
    them only when they are ASCII digits.
  - Test: `dates_that_do_not_start_with_a_year_are_left_out_not_cut` (crates/sonora-engine/src/export/id3.rs); `dj_export_writes_tagged_copies_and_a_rekordbox_playlist` exports a FLAC dated `二〇一九` (crates/sonora-core/tests/dj.rs).

- [x] **B105. In the macOS app a Soulseek preview never plays: it says "Enough has arrived for now", and there is no sound.**
  Reported by a listener after the review.
  - Symptom: Preview shows the spinner, the status goes to "Enough has
    arrived for now", and nothing is heard, for any file.
  - Steps: in the macOS app, preview a FLAC from someone's share.
  - Cause: macOS's player (AVFoundation, under WebKit) asks for the last
    64 KB of a file before it starts, and buffers a while before it plays.
    The preview's end had not arrived, so that read waited for it; and the
    core fetched only 60 s ahead of the playhead, which never moved, so the
    transfer paused there and the end never came. Each waited for the
    other. Chromium (Windows, the browser build, the tests) reads from the
    start on and never asked for the end, which is why no test saw it.
  - Fix: a read of the end of the file, far beyond what has arrived, is
    answered at once with silence; a read that waits keeps the transfer
    going, playhead or not; and the core reads ahead of what the player has
    read, not only of what it plays. A range with an end (AVFoundation asks
    for those) is answered whole, as a finished file would be, never with
    the part that happened to be there. The desktop app and the dev bridge
    answer the player with the same code in the core (`Previews::answer`).
    While the player waits for audio, the status says "Buffering…".
  - Test: `the_macos_player_is_answered_though_it_reads_the_end_first_and_buffers_before_it_plays` (crates/sonora-core/tests/preview.rs) replays AVFoundation's reads; it hung on the end of the file before the fix, on the reads past 20 s with only that part fixed, and got short answers without the last part.

## High

- [x] **B03. A second playlist started while another works is left "running" with nothing running it.**
  - Symptom: Start, Retry, Resume, "Download picks" or choosing a file on
    playlist B while A is running or paused says "Another playlist is being
    searched", but B has already been saved as running (and its not-found
    tracks reset). B then shows Running forever; Resume says it is not
    paused, Start says it is running, and only Stop gets out.
  - Steps: fetch two playlists, start A, pause it, start B; reopen B.
  - Cause: `start`, `resume_run`, `confirm`, `choose` and `retry` wrote the
    new state before `spawn` refused, and nothing undid it.
  - Fix: refuse before writing anything, under the lock `spawn` uses.
  - Test: `one_run_works_at_a_time_and_the_others_are_left_as_they_were` (crates/sonora-core/tests/playlist.rs).

- [x] **B04. "Set up Soulseek" switches "Share a folder" back on.**
  - Symptom: with an earlier slskd that shared a folder, the person turns
    sharing off, then clicks **Stop it** (another slskd) or **Quit the
    Soulseek app**. The switch comes back on with the old folder, and
    **Allow and set up** writes that share into slskd.yml.
  - Cause: every refresh of the setup plan after an action re-applied the
    found share, including `setSharing(true)`.
  - Fix: take the found share only on the first look after the dialog opens.
  - Test: e2e “Soulseek: one button sets slskd up…”: the share, turned off, stays off after “Stop it”.

- [x] **B05. Stopping a USB send drops tracks from the drive's playlists.**
  - Symptom: after a send that would update every file (a new key notation,
    say) is stopped after the first file, the M3U, the Serato crate and
    rekordbox.xml list that one track; the other files are still there.
  - Cause: only files handled before the stop had a place in the playlist
    files; files waiting for an update had none, although their old copy
    was untouched.
  - Fix: after a stop, every update not reached keeps its old file in the
    playlists, as a failed update already did.
  - Test: `a_stopped_send_and_a_changed_playlist_file_leave_the_drive_in_order` (crates/sonora-core/tests/usb.rs).

## Medium

### DJ export and Send to USB

- [x] **B06. MP3 export warned about a resolution choice it does not have** (the known bit-rate bug).
  - Symptom: exporting a 24-bit or high-rate source as MP3 320 showed "1 file
    with notes: Reduced to 16-bit/44.1 kHz for compatibility (choose high
    resolution to keep more)" or not, depending on the Quality last chosen
    for AIFF/WAV, which the dialog hides for MP3.
  - Steps: Export… a 24-bit WAV, pick AIFF and 16-bit / 44.1 kHz, then MP3
    320.
  - Cause: the dialog still sent the hidden `quality`, and `warnings_for`
    applied the AIFF/WAV rule to MP3. AIFF and WAV were checked the same way
    and are right (16/24-bit, 44.1/48 kHz, as chosen).
  - Fix: the dialog sends `universal` for MP3, and the core gives MP3 no
    resolution warning whatever it is sent.
  - Test: `mp3_exports_carry_no_resolution_warning_and_pcm_bit_rates_are_rounded` (crates/sonora-core/src/djexport.rs); e2e “the export plan follows the format: MP3 never warns about a resolution choice it does not show”.

- [x] **B07. rekordbox.xml gave copied MP3s no length.**
  - Symptom: `TotalTime="0"` for every MP3 source exported as MP3 (copied
    frame for frame), so rekordbox lists them as 0:00 until it analyses
    them.
  - Cause: a copied MP3 has no decoded duration; the export used that zero.
  - Fix: fall back to the length the library measured.
  - Test: `dj_export_prefers_the_best_copy_and_encodes_mp3` checks `TotalTime` and `BitRate` (crates/sonora-core/tests/dj.rs).

- [x] **B08. Changing the hidden Quality re-sends every MP3 on a USB drive.**
  - Symptom: an MP3 send that differs from the last one only in the Quality
    hidden for MP3 plans every file as "update" and rewrites the drive.
  - Cause: `content_key` hashed the quality for MP3 too.
  - Fix: quality counts for AIFF and WAV only; the dialog also sends
    `universal` for MP3.
  - Test: `mp3_files_do_not_depend_on_the_hidden_quality_and_kinds_follow_each_file` (crates/sonora-core/src/usb.rs).

- [x] **B09. The USB dialog stays on "An export or another send is running".**
  - Symptom: opened while an export or another send runs, Send stays
    disabled after that job ends, until an option is changed.
  - Cause: the dialog never planned again when a job ended.
  - Fix: plan again on `djexport.finished` and on a `usb.finished` it is not
    waiting for, as the export dialog does.
  - Test: e2e “review: the USB dialog plans again when another job ends…”.

- [x] **B10. A playlist file the person changed on the drive gets a new number on every send.**
  - Symptom: after editing `Warm-up.m3u8` (or Serato rewriting a crate),
    each send writes `Warm-up (2).m3u8`, then `(3)`, then `(2)` again, with
    a "not SONORA's" warning every time.
  - Cause: the free-name search skipped SONORA's own `(2)` from the last
    send, and the clean-up then removed it.
  - Fix: reuse SONORA's intact earlier variant of that name before looking
    for a new one.
  - Test: `a_stopped_send_and_a_changed_playlist_file_leave_the_drive_in_order` (crates/sonora-core/tests/usb.rs).

- [x] **B11. Files written just before an interruption become "not SONORA's".**
  - Symptom: after the app quits or the stick is pulled during a send, up to
    nine files SONORA wrote are reported as someone else's on the next send
    and duplicated as `… (2).aiff`.
  - Cause: the drive's record was saved every 10 files.
  - Fix: save it after every file (it is small).
  - Test: `a_stopped_send_and_a_changed_playlist_file_leave_the_drive_in_order` (crates/sonora-core/tests/usb.rs).

- [x] **B12. Quitting during an export or a send leaves hidden partial files, and can hide a replaced track.**
  - Symptom: `.name.sonora-part` files stay in the export folder or on the
    stick; a track being replaced stays as `.sonora-old-N`.
  - Cause: `App::shutdown` only flagged these jobs and returned; the process
    ended before they cleaned up.
  - Fix: shutdown waits (up to a few seconds, shared) for the export, the
    send and the other background jobs to stop.
  - Test: `quitting_during_a_send_lets_it_put_the_drive_in_order` (crates/sonora-core/tests/usb.rs).

### YouTube playlist downloads

- [x] **B13. A track stays "checking" for ever when it is removed or never analysed.**
  - Symptom: the run never finishes and never says so.
  - Steps: remove a playlist track from the library while it is being
    checked, or cancel the analysis it waits for.
  - Cause: no time limit after the file was found, and "the track is gone"
    was not an outcome.
  - Fix: a removed track counts as done with a note; a check that waits too
    long gives up with a reason.
  - Test: `a_run_gets_over_slskd_restarting_and_a_track_removed_while_checked` (crates/sonora-core/tests/playlist.rs).

- [x] **B14. "Could not reach slskd" stays on a run after slskd is back.**
  - Cause: the error was written when a search failed and never cleared.
  - Fix: clear it when a search works again.
  - Test: `a_run_gets_over_slskd_restarting_and_a_track_removed_while_checked` (crates/sonora-core/tests/playlist.rs).

- [x] **B15. Choosing another file while the run checks a download can be overruled.**
  - Symptom: "You chose X… X did not work out (the download was cancelled);
    trying the next", and a different file downloads.
  - Cause: the run's fallback did not check that the item was still the one
    it had looked at.
  - Fix: the fallback and the transfer updates apply only to the transfer
    they examined.
  - Test: `a_verdict_on_an_earlier_pick_does_not_overrule_the_persons_choice` (crates/sonora-core/src/jobs.rs).

- [x] **B16. The rules bar on a paused or reviewed run changes nothing for that run.**
  - Symptom: the bar shows on a paused run and on review, but the run keeps
    the rules it started with.
  - Fix: resuming a paused run takes the current rules (they apply from the
    next search on); the bar is not shown on review, where the picks are
    already made.
  - Test: `a_playlist_is_read_searched_downloaded_and_checked` changes the rules while paused (crates/sonora-core/tests/playlist.rs). That the bar is hidden on review was checked by hand.

- [x] **B17. Ticking a left-out video in a finished run leaves it waiting for ever.**
  - Fix: left-out videos can only be ticked while the run can still search
    them (draft, running, paused).
  - Test: `src/lib/playlist.test.ts`.

- [x] **B18. "Largest file" and the rejection reasons count MiB but say MB.**
  - Symptom: with a 50 MB limit a file listed as 52.0 MB passes; a rejected
    one shows 60.0 MB in its row and "57.2 MB is above the limit" in its
    reason.
  - Fix: decimal megabytes everywhere, as the rest of SONORA uses.
  - Test: `sizes_are_decimal_megabytes_as_everywhere_else` (crates/sonora-core/src/matching.rs).

### Soulseek

- [x] **B19. One failed poll stops a search for good.**
  - Symptom: "Searching… N people answered so far" never moves on; Stop
    search does nothing visible.
  - Cause: the next poll was only scheduled after a successful one.
  - Fix: keep polling with a back-off after a failure, and poll once more
    after Stop.
  - Test: e2e “Soulseek: search through slskd…”: the first look at the results fails, and the results still come.

- [x] **B20. Downloads that match files already in the folder read "Added, then removed from the library".**
  - Symptom: after choosing a folder that already held slskd's earlier
    downloads, the Downloads tab labels each of them "Added, then removed";
    a playlist download with the same name and size waits for ever.
  - Cause: the match against the folder's files included files noted as
    already there before the folder was chosen.
  - Fix: leave those out of the match.
  - Test: `a_finished_download_is_not_matched_to_a_file_that_was_there_before` (crates/sonora-core/src/slskd.rs).

- [x] **B21. A preview that follows a download never ends when the download is cancelled.**
  - Fix: a cancelled transfer ends any preview reading it.
  - Test: `a_preview_of_a_download_that_is_cancelled_ends_where_the_audio_stops` (crates/sonora-core/tests/preview.rs).

- [x] **B22. "Remove what the setup made" leaves SONORA's settings pointing at what it removed.**
  - Symptom: the Soulseek page then says the download folder is missing and
    searches fail at the address the setup wrote; an earlier slskd address
    is lost.
  - Fix: the setup remembers the settings it changed and puts them back
    when its files are removed (unless the person changed them since).
  - Test: `removing_what_a_setup_made_puts_settings_back_and_repair_keeps_the_rest_of_slskd_yml` (crates/sonora-core/tests/slskd_service.rs).

- [x] **B23. Repair, or running the setup again, rewrites slskd.yml from scratch when one expected line is missing.**
  - Symptom: every setting the person added in slskd.yml (other API keys,
    limits, shares) is gone; only a dated backup keeps them.
  - Cause: the YAML writer could only replace values that exist; anything
    missing fell back to writing a whole new file.
  - Fix: missing keys are added in place; the rest of the file stays.
  - Test: the same test, and `missing_keys_are_added_where_they_belong_and_nothing_else_moves` (crates/sonora-core/src/yaml.rs).

### Library, analysis and scans

- [x] **B24. A row clicked right after typing in the search box is not selected.**
  - Symptom: the row lights up, then the selection and the details panel
    vanish; a click a little later lands on an empty table.
  - Steps: type in the library search and click a row within about 200 ms.
  - Cause: when the search applied, the library cleared the selection and
    the table blanked until the new results came.
  - Fix: a search keeps the selected tracks that are still in the results,
    and the table shows the previous rows until the new ones arrive.
  - Test: e2e “review: the library keeps the selection through a search…”.

- [x] **B25. Clear right after typing leaves the table filtered by text no longer shown.**
  - Cause: the delayed search from the last keystroke ran after Clear.
  - Fix: Clear cancels it.
  - Test: e2e “review: the library keeps the selection through a search…”.

- [x] **B26. Clicking a row's ▶ takes the keyboard away from the table** (Windows, Linux).
  - Fix: the button does not take focus on click.
  - Test: e2e “review: the library keeps the selection through a search…”.

- [x] **B27. A failed re-analysis is hidden on the track page.**
  - Symptom: the page keeps the old scores with no sign that the latest
    analysis failed or why.
  - Fix: a notice above the results says it failed, why, when the shown
    results are from, and offers Try again.
  - Test: e2e “review: a failed re-analysis says so above the earlier results”.

- [x] **B28. Analysis history shows at most 500 results of a scan.**
  - Symptom: "The full list is in Analysis history" links to the same 500.
  - Fix: history pages through all results (Show more).
  - Test: none: it takes a scan of more than 500 files, and the test corpus has about 40. Checked by reading: the page asks `scan.items` for the next 500 from where it is.

- [x] **B29. Tracks added to a running scan are analysed one at a time.**
  - Symptom: the status says 4 workers, one is working, and the time left
    is a quarter of the real one.
  - Cause: workers that found the queue empty ended and were not started
    again when more tracks came.
  - Fix: start workers again for new tracks; the count and the time left
    use the workers alive.
  - Test: `tracks_added_to_a_running_scan_get_every_worker` (crates/sonora-core/tests/library.rs).

- [x] **B30. Cancel does not stop an import waiting behind the one being read.**
  - Symptom: the second folder is read and analysed after Cancel.
  - Cause: each import cleared the shared cancel flag when it started.
  - Fix: Cancel stops every import started before it.
  - Test: `cancel_stops_imports_waiting_their_turn_and_folders_not_read_yet` (crates/sonora-core/tests/library.rs).

- [x] **B31. The Problems, Clipping and Lossy source tabs of a scan list tracks it never analysed.**
  - Fix: those tabs count only tracks the scan finished.
  - Test: `a_scans_problem_tabs_list_only_what_it_analysed_and_a_pause_is_not_its_time` (crates/sonora-core/tests/library.rs).

- [x] **B32. A file that disappears during a scan turns a done track into "failed".**
  - Symptom: an analysed track shows Failed ("no longer exists"), counts as
    a failure on disk, and Retry fails again.
  - Fix: a missing file is marked missing and keeps its earlier analysis.
  - Test: `a_file_gone_before_its_turn_is_missing_not_failed` (crates/sonora-core/tests/library.rs).

- [x] **B33. A truncated MP3 is called clean.**
  - Symptom: an MP3 cut short (an incomplete download) reads "High-quality
    MP3 at 320 kbps. No other technical issues detected."
  - Cause: the MP3 path read the frame count from the Xing/Info header but
    never used it as the declared length, so the truncation check never
    ran.
  - Fix: the declared length comes from the Xing/Info/VBRI frame count.
  - Test: `real_lossy_encodes_and_transcodes` cuts an MP3 short (crates/sonora-engine/tests/signals.rs).

- [x] **B34. A lossless 32 kHz file is called "likely transcoded from a ~96–112 kbps lossy source".**
  - Cause: the cut-off rules use fixed frequencies tuned for 44.1/48 kHz, so
    the normal anti-alias edge just below 16 kHz counts as an encoder
    cut-off.
  - Fix: an edge in the anti-alias band just below the file's top frequency
    is not a lossy cut-off.
  - Test: `a_low_sample_rate_is_not_taken_for_a_lossy_source` (crates/sonora-engine/tests/signals.rs).

- [x] **B35. MP3 or AAC inside a WAV is called heavily clipped.**
  - Symptom: the same stream scores 75 as .mp3 and 43 with "Heavy hard
    clipping" when wrapped in WAV.
  - Cause: WAV/AIFF/FLAC containers were always decoded to integers, which
    clamps a lossy decoder's overshoot into flat runs.
  - Fix: a payload known to be lossy is decoded as a lossy source whatever
    the container.
  - Test: `an_mp3_inside_a_wav_is_judged_as_the_mp3_it_is` (crates/sonora-engine/tests/signals.rs).

- [x] **B36. Opus and Vorbis bit rates include the cover art.**
  - Symptom: an 88 kbps Opus with a 600 kB picture reads 195 kbps.
  - Fix: the bit rate counts the audio pages only.
  - Test: `ogg_opus` with a large picture (crates/sonora-engine/tests/meta_formats.rs).

- [x] **B37. "Audio is 2 min long, above the 2 min analysis limit" for a 5-minute file.**
  - Fix: the message gives the real length when the file declares one, and
    says "longer than" otherwise.
  - Test: `a_file_over_the_length_limit_says_how_long_it_is` (crates/sonora-engine/tests/signals.rs).

- [x] **B38. A truncated WAV shows "Declared 0:10" for a 0:20 header.**
  - Fix: the declared length comes from the header, not from what is there.
  - Test: `detects_truncation` (crates/sonora-engine/src/decode/wav.rs).

- [x] **B39. iTunes' "iTunNORM" numbers show as the comment, and go into exports.**
  - Fix: comments made by iTunes for its own use are skipped.
  - Test: `itunes_numbers_dlis_and_genre_numbers` (crates/sonora-engine/src/meta/id3.rs).

### Playlists and DJ tools

- [x] **B40. A playlist keeps tracks selected after they leave it.**
  - Symptom: after an edit moves tracks out of a smart playlist, the panel
    still offers to remove, trash or edit "2 tracks" no longer shown.
  - Fix: the selection keeps only tracks still in the playlist.
  - Test: e2e “review: smart playlists keep their rules as saved…”.

- [x] **B41. A smart playlist's genre outside the 60 most common is invisible in Edit rules and cannot be removed.**
  - Fix: chosen genres always show (and can be turned off), and the label
    lists them.
  - Test: e2e “review: smart playlists keep their rules as saved…”.

- [x] **B42. "Save as smart playlist" from library filters matches more tracks than the library showed.**
  - Symptom: filtered to 120–125 BPM the library leaves out 125.4; the saved
    smart playlist includes it.
  - Cause: the rules dialog reads "to 125" as up to 125.999, the library
    filter as 125.
  - Fix: the rules dialog keeps a tempo range it was given as it is, unless
    it is edited there (then "to 125" includes 125.9, as before).
  - Test: e2e “review: smart playlists keep their rules as saved…”.

- [x] **B43. Emptying a field in Edit BPM, key and genre says "Updated 1 track" and the value comes back.**
  - Cause: an empty field only removes an earlier edit; with no edit there
    was nothing to change.
  - Fix: emptying a field that holds no edit is not a change (Save stays off
    for it), and the field says what an empty value goes back to.
  - Test: e2e “review: the library keeps the selection through a search…”.

- [x] **B44. A smart playlist's description misses rules saved from the library.**
  - Symptom: "All tracks" for a playlist of clipped tracks; "score 35+" for
    35–54; a score other than DJ called "score".
  - Fix: the description covers every rule, with the same words as Edit
    rules.
  - Test: `src/lib/rules.test.ts`.

- [x] **B45. Sort into playlists does not mention what it left out.**
  - Fix: the message says how many genres were left out (beyond 200
    playlists) and how many tracks were skipped because their file is
    missing.
  - Test: `src/lib/rules.test.ts`; `smart_playlists_sorting_into_playlists_and_arranging` counts the missing tracks (crates/sonora-core/tests/playlists.rs).

### Settings

- [x] **B46. Clearing a number field saves its minimum.**
  - Symptom: clearing "Longest file analyzed" and leaving the field saves
    1 minute, and every longer file then fails. Confirmed on the fixtures
    library (240 → 1).
  - Cause: `Number('')` is 0, which passed the check and was clamped up.
  - Fix: an empty field goes back to the saved value.
  - Test: `src/lib/stepper.test.ts`; e2e “review: the USB dialog plans again…” empties a field in Settings.

## Low

### DJ export and Send to USB

- [x] **B47. PCM bit rates were cut, not rounded, in rekordbox.xml** (24-bit / 44.1 kHz gave 2116 instead of 2117).
  - Test: `mp3_exports_carry_no_resolution_warning_and_pcm_bit_rates_are_rounded` (crates/sonora-core/src/djexport.rs).
- [x] **B48. The export dialog does not say a USB send is running;** Export then fails with a toast. Fix: the plan reports either job as running, and plans again when it ends.
  - Test: `an_export_plan_says_when_a_send_is_running` (crates/sonora-core/tests/usb.rs).
- [x] **B49. The drive's rekordbox.xml labels files of other formats with the format of this send** (`Kind="AIFF File"` for an MP3). Fix: Kind from each file's extension.
  - Test: `mp3_files_do_not_depend_on_the_hidden_quality_and_kinds_follow_each_file` (crates/sonora-core/src/usb.rs).
- [x] **B50. "Show in Finder" on the export and USB result pages does nothing when it fails.** Fix: say why, as elsewhere.
  - Test: e2e “Send to USB writes numbered folders…” makes Reveal fail.
- [x] **B51. A failed USB send ends "…in the log.." and still shows how to use the drive.** Fix: one full stop; the how-to only when something is on the drive.
  - Test: e2e “review: the USB dialog plans again…”.
- [x] **B52. Arrange for mixing draws the current set squashed until the result arrives, and keeps "Working it out…" after an error.** Fix: measure the width when the dialog shows; no placeholder after an error.
  - Test: none (a layout timing); checked by reading.

### YouTube playlist downloads and Soulseek

- [x] **B53. A not-found track shows a rejected file in the File column as if it were the pick.**
  - Test: `src/lib/playlist.test.ts`.
- [x] **B54. The bin on a run waiting to look again always fails** ("Stop the run before removing it"). Fix: removing a waiting run stops it first.
  - Test: `one_run_works_at_a_time_and_the_others_are_left_as_they_were` (crates/sonora-core/tests/playlist.rs).
- [x] **B55. Start on a run waiting to look again does nothing for up to 30 s.** Fix: Start wakes the run.
  - Test: `one_run_works_at_a_time_and_the_others_are_left_as_they_were` (crates/sonora-core/tests/playlist.rs).
- [x] **B56. Download speeds under 1 kB/s show raw digits** ("523.4567891 B/s"), and sizes round up to "1000 kB". Fix: `formatBytes` rounds and moves up a unit at 1000.
  - Test: `src/lib/format.test.ts`.
- [x] **B57. "Highest resolution taken" shows "24, 96000".** Fix: "24-bit, 96 kHz".
  - Test: e2e “review: the USB dialog plans again…” reads the rule in Settings.
- [x] **B58. A folder whose files are all in the library says "All asked for".** Fix: "All in your library".
  - Test: e2e “Soulseek: search through slskd…”.
- [x] **B59. Overlapping slskd connection tests can overwrite a newer answer, and the Test buttons spin every 6 s.** Fix: drop stale answers; skip a tick while a test runs.
  - Test: none: it needs slskd to answer two tests out of order; checked by reading.
- [x] **B60. Changing the Soulseek password closes the form and drops what was typed even when it fails.** Fix: close only on success.
  - Test: none: it needs SONORA's own slskd to refuse a password; checked by reading.
- [x] **B61. The Downloads tab is blank after "Make a key" until the connection is tested.** Fix: it tests, and says so meanwhile.
  - Test: none: the e2e tests set slskd up in one go; checked by reading.

### Library, analysis and scans

- [x] **B62. "Clear" (filters) drops keyboard focus to the page.** Fix: focus moves to the search field.
  - Test: e2e “review: the library keeps the selection through a search…”.
- [x] **B63. A context menu closed by scrolling or resizing leaves focus on the page.** Fix: focus returns to where it was.
  - Test: e2e “review: the library keeps the selection through a search…”.
- [x] **B64. With nothing highlighted, ↑ in a menu goes to the second-to-last item.** Fix: it goes to the last.
  - Test: e2e “review: the library keeps the selection through a search…”.
- [x] **B65. "1 tracks", "1 events", "1 samples".** Fix: singular forms.
  - Test: `plural()` in `src/lib/format.test.ts`; the places that use it were checked by hand.
- [x] **B66. The Folders "Problems" count counts a failed, missing track twice.**
  - Test: `a_file_gone_before_its_turn_is_missing_not_failed` (crates/sonora-core/tests/library.rs).
- [x] **B67. The details panel says "Cutoff: None detected" for tracks never analysed.** Fix: "—".
  - Test: e2e “review: the library keeps the selection through a search…”.
- [x] **B68. A track being re-analysed never shows as analysing.** Fix: the core says when a track starts, and the table and track page show it.
  - Test: `tracks_added_to_a_running_scan_get_every_worker` (crates/sonora-core/tests/library.rs).
- [x] **B69. A scan cancelled while paused reports the paused time as its length.**
  - Test: `a_scans_problem_tabs_list_only_what_it_analysed_and_a_pause_is_not_its_time` (crates/sonora-core/tests/library.rs).
- [x] **B70. A scan interrupted by a crash shows the time until the next launch as its length.** Fix: no length for an interrupted scan without a summary.
  - Test: none: it needs the core to crash mid-scan; checked by reading.
- [x] **B71. A cancelled import of several folders still adds the folders not yet read.**
  - Test: `cancel_stops_imports_waiting_their_turn_and_folders_not_read_yet` (crates/sonora-core/tests/library.rs).
- [x] **B72. Removing a folder (keeping its tracks) while it is being read fails with a database error.** Fix: refused as busy until the reading ends.
  - Test: `cancel_stops_imports_waiting_their_turn_and_folders_not_read_yet` (crates/sonora-core/tests/library.rs).
- [x] **B73. Library search ignores case only for ASCII letters** ("ночь" does not find "Ночь"). Fix: SONORA folds case itself.
  - Test: `search_ignores_case_in_every_script` (crates/sonora-core/src/library.rs).
- [x] **B74. The player's total time floors while lists round** (5:59 against 6:00). Fix: the total rounds like the lists.
  - Test: `formatDuration(359.6)` in `src/lib/format.test.ts`.
- [x] **B75. Times round up into the next unit** ("about 1 h 60 min", "60 min ago", "24 h ago").
  - Test: `src/lib/format.test.ts`.
- [x] **B76. Partial decoded playback copies are never removed** (a quit while a track is being prepared). Fix: stale `.part` files are removed.
  - Test: `playback_uses_the_original_or_one_decoded_copy` (crates/sonora-core/tests/dj.rs).
- [x] **B102. Cancel during the import of a folder holding thousands of files waits until every file is read** (minutes on Windows). Found by CI after the review. Fix: the folder walk looks at Cancel between files, not only between folders.
  - Test: `a_cancelled_walk_stops_inside_a_big_folder` (crates/sonora-core/src/index.rs).
- [x] **B104. On Duplicates, Remove from library and Move to Trash are cut off, shown as "…".** Found in the design pass. The actions column was narrower than its five buttons. Fix: a wider column that never cuts its buttons short.
  - Test: e2e “duplicates: …” checks that every row's buttons fit their cell.

### Analysis engine

- [x] **B77. "This at 320 kbps file has no content above 16.8 kHz".** Fix: "This 320 kbps MP3 file…".
  - Test: `real_lossy_encodes_and_transcodes` (crates/sonora-engine/tests/signals.rs).
- [x] **B78. Files shorter than one analysis block get findings that contradict themselves** (an 8 ms burst at -9 dBFS "never rises above -60 dBFS"). Fix: the last partial block counts; no cut-off finding without spectrum frames.
  - Test: `very_short_sounds_are_not_called_silent_or_cut_at_0_khz` (crates/sonora-engine/tests/signals.rs).
- [x] **B79. ID3v2.4 frames with a data-length indicator read with junk in front** ("\u{3}Night Drive").
  - Test: `itunes_numbers_dlis_and_genre_numbers` (crates/sonora-engine/src/meta/id3.rs).
- [x] **B80. Numeric ID3 genres show raw** ("(17)" instead of "Rock").
  - Test: `itunes_numbers_dlis_and_genre_numbers` (crates/sonora-engine/src/meta/id3.rs).
- [x] **B81. Latin-1 RIFF INFO and AIFF text chunks show "�".** Fix: read as Latin-1 when they are not UTF-8.
  - Test: `itunes_numbers_dlis_and_genre_numbers` (crates/sonora-engine/src/meta/id3.rs).
- [x] **B82. The FFmpeg watchdog could underflow and stop a healthy decode.** Fix: saturating subtraction.
  - Test: `activity_noted_just_after_the_clock_was_read_is_not_a_stall` (crates/sonora-engine/src/decode/ffmpeg.rs).

### Settings, updates and other

- [x] **B83. A failed automatic update check hides an update already found.** Fix: the offer stays.
  - Test: e2e “updates: a failed automatic check keeps the offer…”.
- [x] **B84. "Check again" (FFmpeg) restarts the update timers and sends an extra update check.** The privacy text promises checks only at launch and every six hours. Fix: the timers depend on the update setting only.
  - Test: e2e “updates: a failed automatic check keeps the offer…”.
- [x] **B85. "Check again" for FFmpeg says nothing when it is still missing.**
  - Test: e2e “updates: a failed automatic check keeps the offer…”.
- [x] **B86. A value typed in a number field is lost when − or + is pressed before it is saved.** Fix: the buttons step from what is typed.
  - Test: e2e “review: the USB dialog plans again…” steps a typed value in Settings.
- [x] **B87. Two settings changed in quick succession can undo each other.** Fix: the core applies settings changes one at a time.
  - Test: `two_settings_changed_at_once_both_stay` (crates/sonora-core/tests/library.rs).
- [x] **B88. One unknown or out-of-range rule resets every saved playlist download rule.** Fix: on load, a bad rule is skipped and the others kept.
  - Test: `saved_rules_keep_what_this_version_takes` (crates/sonora-core/src/settings.rs).
- [x] **B89. Some error messages begin with "CoreError:".**
  - Test: none; the four places now read the message as every other toast does.
- [x] **B90. Cancelling a Discogs lookup during a rate-limit wait reports a rate-limit error,** and a cancel counts a track as not found.
  - Test: `a_discogs_lookup_stopped_while_it_waits_says_it_was_stopped` (crates/sonora-core/tests/dj.rs).
- [x] **B91. Untagged files of artists whose names start with digits are looked up wrongly** ("808 State - Pacific" as "State").
  - Test: `titles_and_file_names` (crates/sonora-core/src/discogs.rs).
- [x] **B92. The CSV export shows accented titles garbled in Excel.** Fix: the file starts with a byte-order mark, as Excel expects for UTF-8.
  - Test: `escaping_and_formula_guard` (crates/sonora-core/src/export.rs) and the CSV check in `query_search_sort_filter_export_and_duplicates` (crates/sonora-core/tests/library.rs).
- [x] **B93. Add to playlist says "All already in …" when nothing was added because the tracks are gone.**
  - Test: none: it needs tracks removed between choosing them and adding them; checked by reading.
- [x] **B94. Edit rules first shows the previous playlist's "N tracks match".**
  - Test: e2e “review: smart playlists keep their rules as saved…”.
- [x] **B95. Taking tracks out of a playlist leaves them on screen until the reload.**
  - Test: e2e “review: smart playlists keep their rules as saved…”.
- [x] **B103. A value typed in a number field, then stepped with +, can end up saved as the typed value.** Found by an e2e test after the review. The two saves could reach the core in either order. Fix: the interface sends settings changes one at a time, in the order they were made.
  - Test: e2e “review: the USB dialog plans again…” slows the first save down; the second still wins.

### Browser build, tests and tooling

- [x] **B96. In the browser build a lost `*.finished` event leaves an export, send, lookup or playlist run showing as running until a reload.** Fix: the jobs are asked again when the event stream reconnects.
  - Test: e2e “the browser build reads the jobs again when its event stream reconnects”.
- [x] **B97. The browser build and `sonora version` report 0.1.0.** Fix: one workspace version.
  - Test: “the manifests agree on one version” (`scripts/release.test.ts`) now includes the workspace Cargo.toml.
- [x] **B98. In CI, a retry of the serial e2e suite always fails on its first test** (the library is no longer empty). Fix: the first test starts from an empty library on a retry.
  - Test: the CI run itself.
- [x] **B99. Two e2e filter checks pass when the filter returns no rows.**
  - Test: the two checks now fail on an empty result.
- [x] **B100. Without FFmpeg, `sonora fixtures` writes no manifest, and the copied WAV is listed as different audio although it is the same bytes.**
  - Test: `without_ffmpeg_the_manifest_still_lists_every_file` (crates/sonora-cli/src/fixtures.rs).
- [x] **B101. The desktop shell's unit tests never run.** Fix: CI runs them.
  - Test: the CI run itself.

## Checked and not a bug, or not confirmed

- Every Rust ↔ TypeScript shape and all 22 core events were compared: they
  match.
- AIFF and WAV exports: 16/24-bit and 44.1/48 kHz as chosen, the bit rate
  and length right in rekordbox.xml.
- No console errors or warnings in any view; the Rust log holds only the
  expected warnings for the three broken fixture files.
- An unreadable file could not be tested here (the review ran as root,
  which reads any file).
- `other_track` reads as 9B in the 45-second fixtures and 8A in the
  20-second e2e corpus: key detection on synthetic audio, not a defect
  found in the code.
- The Discogs token is kept in the settings table, on this computer: by
  design, not a finding.
- Not confirmed, so not changed: 7.1 loudness weights, AIFF-C compressed
  frame counts, WAV files over 4 GB.
