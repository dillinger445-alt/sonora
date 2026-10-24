# Changelog

What changed in each version of SONORA. The section of a version is also
what the app shows when it offers that version as an update, so it is
written for the people using SONORA.

## Unreleased

### New

- **Download a YouTube playlist from Soulseek.** On the Soulseek page,
  **YouTube playlist** takes the link of a public or unlisted playlist,
  reads its track list (titles read as artist, title and mix name; Shorts,
  mixes and repeats left out, with a tick to put any back), looks for every
  track through slskd, one search at a time with up to five searches each,
  picks the best file by the rules under Settings → **Playlist download
  rules** (formats in order, resolution, size, who shares it, how well the
  name matches) and downloads it, automatically or after you review the
  picks. A file that arrives made from a lossy source, damaged or lossy is
  swapped for the next best, up to three tries. The run can be paused,
  stopped and retried, shows in the status bar, and carries on after a
  restart. YouTube sees the playlist's id and nothing about you.

### Changed

- **A new icon.** SONORA's icon is now a meter whose scale is a sound wave,
  lime on graphite, drawn simpler at small sizes so it stays readable down
  to 16 pixels; the sidebar's wordmark carries it too.
- **Faster with large libraries.** With thousands of tracks, a running scan
  no longer redraws every row of the library on each update (about 93 %
  less drawing), long playlists open in about half the time and move with
  the arrow keys without lag, their set timeline fits the window, and an
  export of thousands of tracks is planned about five times faster.
- **Design.** Type sizes and corner radii follow one scale throughout; the
  Theme setting shows its one value instead of a control with one choice.
- **Scores, version 1.1.** A lossless file at a low sample rate (32 kHz,
  say) is no longer called a likely transcode, an MP3 or AAC stream inside
  a WAV is judged as the lossy file it is (not as heavily clipped), and an
  MP3 cut short is no longer called clean. Tracks already analysed are
  scored again from their stored measurements when SONORA starts; the
  changes that need the audio apply when a track is analysed again.

### Fixed

SONORA was reviewed function by function before this version; every
finding is listed, with how to reproduce it and the test that now guards
it, in `docs/BUGS.md`. The ones you could meet:

- **Playlist downloads** arrive in the library again: finished files no
  longer stay on "Arrived; checking it" until SONORA gives up on them. A
  second playlist started while another works waits instead of being left
  "running" with nothing running it; a track removed while it is checked no
  longer holds the run for ever; "Could not reach slskd" goes once slskd is
  back; choosing another file while a download is checked is not
  overruled; rules changed while a run is paused apply when it resumes;
  sizes are decimal megabytes, as everywhere else.
- **DJ export and Send to USB**: a date tag that is not a plain year (a
  Japanese date, say) no longer stops the export with an internal error.
  MP3 exports no longer warn about a resolution choice MP3 does not have,
  and rekordbox gets the length of copied MP3s. A stopped send keeps every
  file it did not reach in the drive's playlists, a playlist file you
  changed on the drive no longer gets a new number on every send, and
  quitting during a send lets it tidy up first. The USB dialog notices when
  an export ends.
- **Soulseek**: "Set up Soulseek" no longer switches sharing back on after
  you turned it off. Repair and the setup add what is missing to
  slskd.yml instead of rewriting it, and removing what a setup made puts
  your earlier settings back. A search keeps going after one failed look;
  a preview ends when its download is cancelled; downloads that match files
  already in the folder are no longer labelled "Added, then removed".
- **Library and analysis**: a row clicked right after typing a search stays
  selected; Clear right after typing really clears; library search ignores
  case in every script ("ночь" finds "Ночь"). Tracks added to a running
  scan are analysed by every worker; Cancel stops imports waiting their
  turn; a file that disappears during a scan is marked missing, not failed;
  a failed re-analysis is shown on the track page; history lists every
  result of a large scan; Opus and Vorbis bit rates leave out cover art;
  iTunes' own comment numbers, numeric ID3 genres and Latin-1 text read
  properly.
- **Playlists**: smart playlists saved from library filters match what the
  library showed; a genre outside the most common ones can be seen and
  turned off; selections follow tracks out of a playlist, and tracks taken
  out go at once.
- **Settings and the rest**: clearing a number field no longer saves its
  minimum, and − / + step from what you typed; two settings changed
  quickly no longer undo each other; one unreadable playlist rule no longer
  resets all of them; a failed automatic update check keeps an update it
  already found; "Check again" for FFmpeg says what it found; the CSV export
  opens with accents intact in Excel; Discogs lookups of artists whose
  names start with digits work, and stopping one says it was stopped.
  A value typed into a number field and then stepped with + is saved as
  stepped; Cancel stops the import of a folder of thousands of files at
  once; on Duplicates, Remove and Move to Trash are no longer cut off.

## 0.3.2

### Fixed

- **Dialogs on a Mac.** Every dialog opened with an empty box: the window
  only darkened, and Esc closed it. So **Set up Soulseek…**, updating to a
  new version, and the questions SONORA asks before removing or changing
  something could not be used. They show again. Windows was not affected.
- **Copy web password** on the Soulseek page works on a Mac. It said it
  could not copy the password: the Mac allows copying only at the moment
  of the click, and the password arrived a moment later.
- **The path of slskd's settings file** on the Soulseek page breaks inside
  its column instead of running over the text next to it.

## 0.3.1

### New

- **Set up Soulseek for me** (macOS): one button on the Soulseek page sets
  up slskd for you. You type your Soulseek user name and password and
  confirm the download folder; one screen lists everything SONORA will do,
  and nothing happens until you allow it. SONORA then downloads slskd from
  its official release on GitHub, checks it against a checksum built into
  SONORA, writes its settings (slskd's web page on this Mac only, a new key
  for SONORA, nothing shared unless you choose a folder), starts it and
  checks that it signs in. Each step is ticked off as it goes, and one that
  fails says why and can be tried again. No Terminal, no keys to copy.
  Settings from an earlier slskd on your Mac are taken over.
- **slskd in the background**: slskd now starts when you log in and again
  if it stops, with no window, whether SONORA is open or not (or, if you
  prefer, only while SONORA is open). The Setup tab shows whether it is
  running and signed in, with Restart, Stop, its log and its web page.
  When it is not signed in, SONORA says why (a wrong password, the same
  account signed in in the Soulseek app, the Soulseek server out of reach)
  with one fix for each. **Repair** puts SONORA's key back if slskd's
  settings lost it. When your Mac wakes from sleep with SONORA open, slskd
  signs in again at once. **Stop** keeps it stopped until you start it;
  **Turn off** also removes it from login, keeping its settings and your
  downloads.
- **Listen before you download**: ▶ beside a FLAC, WAV or AIFF result plays
  it in SONORA's player while it arrives, starting within seconds. A preview
  fetches only a minute or so ahead of what plays, stops fetching when you
  stop listening, and leaves nothing behind: it is never downloaded into
  your library, analysed or shared. Queue up several and listen one after
  another, and choose **Download** in the player to keep one: the download
  carries on from what the preview fetched.

### Fixed

- SONORA and slskd can no longer end up with different API keys: SONORA
  keeps the two in step by itself.
- A second slskd (one started by hand, say) no longer gets in the way
  without a word: SONORA says which one it is and can stop it, with your OK.

## 0.3.0

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
