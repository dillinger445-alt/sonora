# Changelog

What changed in each version of SONORA. The section of a version is also
what the app shows when it offers that version as an update, so it is
written for the people using SONORA.

## Unreleased

### New

- **Soulseek**: a new page for finding lossless music. SONORA searches
  Soulseek through slskd, a free Soulseek client you install and run with
  your own account, and lists FLAC, WAV and AIFF results grouped by folder,
  with the people most likely to deliver soon first. Download a file or a
  whole folder, follow the downloads, and see each finished file's score
  and verdict as it joins your library: a "FLAC" made from an MP3 shows as
  **Lossy source?** straight away. SONORA never connects to Soulseek itself
  and never shares anything; slskd's sharing stays as you set it. Nothing
  happens until you accept the notice on the page. slskd's API key stays
  in the macOS keychain or Windows Credential Manager. The setup guide is
  in docs/SOULSEEK.md.
- **Downloads from any Soulseek client** can join the library: choose the
  folder where it saves finished downloads, and each finished file is added
  and checked, at once or after you review it. Music already in the
  folder when you choose it is left alone unless you add it. SONORA only
  reads that folder; it never moves, renames or deletes anything there.
- A track's analysis and details show when it came from Soulseek.

## 0.2.1

### New

- **Move to Trash**: for files you no longer want. In the details panel,
  the selection bar and a track's page (or **⌘⌫** on a Mac), it moves the
  audio file to the Trash (the Recycle Bin on Windows) and takes its track
  out of the library. It asks first, and you can put the file back from the
  Trash. Files on a drive that is not connected are left alone.
- **Bring back** in Folders returns the tracks you removed from a folder.
- **A menu for every track**: right-click a track in the library or in a
  playlist (or press Shift+F10) to play it, open its analysis, show it in
  Finder or File Explorer, edit it, take it out of the playlist, remove it
  from the library or move its file to the Trash. With several tracks
  selected, the menu acts on all of them.
- On a track's page, the delete key removes that track (after asking).

### Fixed

- On a Mac, the delete key removes tracks and nothing else. Outside a text
  field, the system's web engine could treat it as Back and change the
  screen instead.
- **Removing tracks from the library now lasts.** Rescanning their folder
  used to add them again; now they stay out until you add the files again
  or choose Bring back.
- Removing tracks while some of them are being analyzed removes the others
  and tells you which stayed, instead of doing nothing.
- A failed Send to USB no longer leaves an empty window. If a screen runs
  into a problem, it says so and offers to try again, and the rest of the
  window keeps working.
- Pause, Resume and Cancel in the Import queue, and stopping an export, a
  USB send or a Discogs lookup, now say when they could not act instead of
  appearing to do nothing.
- **Analyze waiting** and **Retry failed** count only files that are still
  on disk.
- Cover images, notes and the `._` files macOS leaves next to music are no
  longer added as tracks when dropped on the window along with music.
- Files added while a cancelled scan is still stopping now wait in the
  queue, and the message says so, instead of announcing an analysis that
  does not start.
- Playback stops when the track playing is removed from the library.
- The details panel of a playlist shows a track's tempo, key and genre
  right after you edit them.
- A smart playlist saved from the library shows its real order in **Edit
  rules**.
- **Convert to a playlist** and **Reset to detected** ask before they drop
  rules or corrections.
- A Discogs token that Discogs refuses is shown as not working, instead of
  saved.
- Backing up the library opens a save window for a library file rather than
  for a CSV file, and can replace an earlier backup you choose there.
- On a Mac, Enter saves the Edit BPM, key and genre window and the smart
  playlist rules again, and after a button in a playlist's details panel
  removes a track, the arrow keys keep moving through the tracks.
- Right-clicking no longer shows the window's own Back and Reload menu,
  which could change the screen or reset the window.
- On Windows, Move to Recycle Bin never deletes a file for good. Files on
  USB sticks, memory cards and network drives, or on a drive whose Recycle
  Bin is turned off or too small for them, stay where they are, and SONORA
  says so.
- The update window warns when installing would stop a USB send.
- Comparing similar recordings no longer fails when tracks are removed
  during the comparison.
- Tempo and key scans have a name in Analysis history.

## 0.2.0

### New

- **Tempo and key** for every analyzed track, in Camelot, Open Key or
  musical notation (sharps or flats), with a BPM range that puts half- and
  double-time readings where you expect them.
- **Filters for DJs**: a tempo range (with half and double tempo), keys
  picked on a Camelot wheel or typed in any notation, keys that mix with
  the one you chose, and genres.
- **Genres** from the tags, from folder names, or looked up on Discogs for
  tracks you choose (optional, with your own free Discogs token).
- **Edit tempo, key and genre** for one track or many; your files are never
  changed.
- **Export for DJ software**: AIFF, WAV or MP3 320 with tempo, key, genre
  and artwork in the tags, the best copy of each recording, and an optional
  rekordbox.xml playlist.
- **Listen inside the app**, with a waveform you can seek in, media keys
  and the tempo and key of the track playing.
- **Playlists and folders** in the sidebar. Drag tracks onto them, drag to
  put them in order, and see each one as a set: its keys, its tempo and how
  every change mixes.
- **Smart playlists** that keep up with the library by themselves: a tempo
  range, keys on the Camelot wheel, genres, DJ score or words.
- **Arrange for mixing** suggests a running order with neighbours in keys
  that mix and close in tempo, and changes nothing until you use it.
- **Sort the library into playlists** by genre, key or tempo in one go.
- **Send to USB**: playlists become numbered folders of DJ-ready files on a
  USB drive, with rekordbox.xml, Serato crates and M3U playlists. Sending
  again brings the drive up to date, and only files SONORA wrote there are
  ever changed.
- **Updates inside the app**: SONORA tells you when a new version is out
  and installs it when you choose, then restarts. Your library, settings and
  edits are kept. Checking can be turned off in Settings.

### Fixed

- Pausing while a track is still loading now sticks, and seeking before it
  starts plays from where you clicked.
- Unmute brings the sound back after the volume slider was pulled down to
  silence.

## 0.1.0

The first version: quality scores for listening and DJing, explained
findings (clipping, loudness, lossy sources, fake high resolution and
more), duplicates, and a library that never touches your files.
