# Soulseek

SONORA can search Soulseek for lossless music, let you listen to a file
before you download it, download what you choose, and add each finished
file to your library with its quality checked, so a "FLAC" made from an
MP3 shows up as soon as it arrives.

SONORA never connects to Soulseek itself. It works through **slskd**, a free
and complete Soulseek client that runs on your computer with your own
Soulseek account. slskd does everything on the network: searching,
downloading and the sharing you set up in it. SONORA sends slskd
instructions (search for these words, download these files) and picks up
the finished files from slskd's download folder. On a Mac, SONORA can set
slskd up and keep it running for you; elsewhere you install and run it
yourself.

- [Before you start](#before-you-start)
- [Setting up on a Mac](#setting-up-on-a-mac)
- [slskd in the background](#slskd-in-the-background)
- [Setting up slskd yourself](#setting-up-slskd-yourself)
- [Connecting SONORA](#connecting-sonora)
- [Searching and downloading](#searching-and-downloading)
- [Downloading a YouTube playlist](#downloading-a-youtube-playlist)
- [Listening before you download](#listening-before-you-download)
- [How downloads join the library](#how-downloads-join-the-library)
- [What SONORA keeps and sends](#what-sonora-keeps-and-sends)
- [Soulseek's rules](#soulseeks-rules)
- [Another Soulseek client](#another-soulseek-client)
- [Troubleshooting](#troubleshooting)
- [Limits](#limits)

## Before you start

The Soulseek page opens with this notice, which you accept once:

> SONORA is for music you have the right to download and use. If you use it
> with Soulseek or another network, you are responsible for making sure each
> download, preview, copy and use is lawful where you live and allowed by
> that network's terms. SONORA does not own, host or license music found on
> other networks, and it never shares your files.

Other Soulseek users see what you search for, along with your Soulseek user
name. SONORA is not affiliated with Soulseek, LLC or with slskd.

If you have never used Soulseek, the first sign-in with a new user name
creates the account. There is no way to reset a forgotten password, so keep
it somewhere safe.

## Setting up on a Mac

On a Mac (Apple silicon or Intel), the **Setup** tab starts with **Set up
Soulseek for me**. **Set up Soulseek…** opens one screen that lists
everything SONORA will do. Nothing is downloaded or written until you
choose **Allow and set up**, and no administrator password is needed:
everything happens in your own user account.

You answer three things:

1. **Your Soulseek user name and password**: the account you use in the
   Soulseek app. It must not be signed in anywhere else while slskd runs.
2. **The folder for finished downloads**: `~/Music/Soulseek` unless you
   choose another. SONORA makes it if it is not there.
3. **Whether to share a folder** with other Soulseek users: off unless you
   turn it on.

Then SONORA:

1. downloads slskd 0.26.1 for your Mac's processor (about 60 MB) from its
   official release on github.com/slskd/slskd;
2. checks the download against the SHA-256 checksum built into SONORA, and
   stops if it differs;
3. unpacks it in `~/Library/Application Support/app.sonora.analyzer/slskd/`;
4. writes slskd's settings (`slskd.yml` in that folder's `data` folder):
   your account, the download folder, the folder to share if you chose one
   (nothing otherwise), slskd's web page on this Mac only with a password
   of its own, and a new API key for SONORA, which SONORA also keeps in your
   keychain;
5. adds a launch agent (`~/Library/LaunchAgents/app.sonora.analyzer.slskd.plist`)
   so slskd starts when you log in and again if it stops. macOS shows a
   notice that slskd can run in the background;
6. starts slskd and waits until it has signed in to Soulseek.

Each step is ticked off as it finishes. At the end the Setup tab says
**Signed in to Soulseek as** your user name, with **Try a search**.

Two things may need your OK on the way, and SONORA asks first:

- **The Soulseek app is open.** One account can be signed in in one place
  at a time. **Quit the Soulseek app** quits it; SONORA never quits it
  without that.
- **Another slskd is running**, one started by hand from Terminal, say.
  Only one slskd can run at a time on a Mac. **Stop it** stops that one.

If the download folder is in Desktop, Documents or Downloads, macOS asks
once whether slskd may use it; choose **Allow**.

SONORA never changes macOS's security settings and never removes the mark
macOS puts on files downloaded from the internet. Its own download of slskd
does not carry that mark, as it does not come through a browser. Should
macOS still stop slskd as a program from an unknown developer, the Start
step says so and opens Privacy & Security in System Settings, where you
can allow it; then choose **Try again**.

### If a step does not work

The step that failed says why and what to do: a password to type again,
the Soulseek app to quit, another slskd to stop, or the network to check.
**Try again** goes on from that step; what was done stays done. **Stop the
setup** stops a setup that is running.

**Remove what the setup made** lists the files and folders this setup made
(the slskd folder, the launch agent, a download folder it created) and
removes them only after you confirm. A download folder is removed only
while it is empty. Nothing else is ever deleted.

### Settings from an earlier slskd

If slskd ran on this Mac before (from a folder on the Desktop such as
`~/Desktop/slskd`, or with slskd's own default settings), the setup screen
fills in the user name, download folder and shared folder from its
`slskd.yml`; reading the one on the Desktop, macOS may ask whether SONORA
may use the Desktop folder. Leave the password empty to keep the one there.
That folder stays as it is; once SONORA's slskd works, you can delete it.
Start an old slskd by hand only after stopping SONORA's (or SONORA says
another slskd is in the way).

Running the setup again, or **Update slskd…**, updates the same setup in
place: the same folder, the same key, the same launch agent. In
`slskd.yml` it changes only the account, the download folder, the shared
folder if you changed it (turning sharing off shares nothing), slskd's web
page on this Mac only and SONORA's key; everything else you set there
stays.

## slskd in the background

Once slskd is set up, the Setup tab shows how it is doing (running, signed
in as whom, or what is wrong) with **Restart** and **Stop** (or **Start**).
**Stop** lasts: slskd stays stopped, also when SONORA opens and when you
log in, until you choose **Start**.

- **Run slskd in the background** (on unless you turn it off): slskd starts
  when you log in and again if it stops, whether SONORA is open or not, so
  downloads go on after you quit SONORA. Off: slskd runs only while SONORA
  is open.
- The launch agent runs slskd with no window and no Terminal, from fixed
  paths, and waits 30 seconds before starting it again after a crash.
- **Show log** shows slskd's log (in the `data/logs` folder), which says
  what it did and why something failed. **Open web page** opens slskd's
  own page, `http://127.0.0.1:5030`, which only this Mac can open: sharing,
  chat and all of slskd's settings are there. **Copy web password** copies
  its password; the user name is `slskd`.
- When SONORA opens, when you open its Soulseek page, and every 20 seconds
  while SONORA runs, it checks that slskd is running and starts it if not,
  unless you stopped it or turned it off.
- When your Mac wakes from sleep while SONORA is open, SONORA asks slskd to
  sign in to Soulseek again at once. Otherwise, and after the network
  drops, slskd tries again by itself, at least every five minutes; **Try
  now** asks at once.

### When something is wrong

When slskd is not signed in, SONORA shows slskd's own reason, from its log,
with one way to fix it:

| What slskd says | The fix |
| --- | --- |
| Soulseek did not accept the user name or password | **Change password**: it goes into `slskd.yml` (after a backup) and slskd starts again |
| The same account signed in somewhere else (the Soulseek app, say) | **Quit the Soulseek app** (only when you choose it), or **Sign in again** |
| slskd could not reach the Soulseek server | slskd keeps trying; **Try now** asks at once |
| slskd's settings hold no key for SONORA, or a different one | **Repair** puts SONORA's key back (after a backup) and starts slskd again |
| Another slskd is running | **Stop it**, then SONORA's starts |

SONORA keeps its key and slskd's in step by itself: when `slskd.yml` holds
a different valid key (after an earlier setup, say), SONORA takes that one
and changes no file. You never copy or paste a key.

Before any change to `slskd.yml`, SONORA copies it to
`slskd.yml.<date and time>.bak` next to it.

### Updating slskd

Each SONORA version names one slskd version, with its checksums. When a
newer SONORA names a newer slskd, the Setup tab offers **Update slskd…**,
with the same screen and the same checks as the setup. SONORA never updates
slskd by itself. Your settings and downloads stay; the older version's
folder (`app-<version>`) stays too.

### Turning it off

**Turn off** stops slskd and removes its launch agent: slskd no longer
starts when you log in or when SONORA opens. slskd's folder, its settings
and your downloads stay where they are (SONORA says where), and **Start**
brings it back. To
remove slskd completely, turn it off, then delete
`~/Library/Application Support/app.sonora.analyzer/slskd/`. It holds slskd
itself, its settings (with your Soulseek password), its log and its partial
downloads; your finished downloads are in their own folder and stay.

## Setting up slskd yourself

On Linux, on Windows (see [Soulseek's rules](#soulseeks-rules)), for slskd
on another computer such as a NAS, or on a Mac if you prefer, choose **I'll
set up slskd myself** on the Setup tab and set it up as follows.

1. **Download slskd** from its [releases page](https://github.com/slskd/slskd/releases):
   `osx-arm64` for a Mac with Apple silicon, `osx-x64` for an Intel Mac,
   `linux-x64` or `linux-arm64` for Linux. Unzip it into a folder of its
   own.
2. **Start it once** from Terminal in that folder with `./slskd`. On a Mac,
   if macOS says it cannot check the app, allow it under System Settings →
   Privacy & Security, then start it again. slskd prints the folder it keeps
   its settings in (the line "Using application directory") and creates
   `slskd.yml` there. Stop it with Ctrl+C.
3. **Edit `slskd.yml`.** The settings SONORA relies on:

   ```yaml
   soulseek:
     username: your-soulseek-name
     password: your-soulseek-password
   directories:
     downloads: /Users/you/Music/Soulseek   # where finished downloads go
   web:
     ip_address: 127.0.0.1                  # slskd's web page on this computer only
     https:
       disabled: true
     authentication:
       username: choose-a-name              # instead of slskd's default slskd/slskd
       password: choose-a-password
   retention:
     search: 1440                           # forget searches after a day
   ```

   The download folder must exist; slskd does not create it. What slskd
   shares is set under `shares:` in the same file. SONORA never changes it;
   see [Soulseek's rules](#soulseeks-rules) on sharing.

   Do not turn on `retention: files: complete`: it makes slskd delete
   finished downloads after a while, including the files your library now
   points to.
4. **Start slskd again** and open its web page, http://localhost:5030. It
   should say it is connected to Soulseek.

To run slskd in the background or start it at login, see the
[slskd documentation](https://github.com/slskd/slskd/tree/master/docs); it
also runs well in Docker on a NAS.

## Connecting SONORA

With slskd set up by hand, go through the rest of **Setup**:

1. **slskd**: links to slskd and to this guide.
2. **API key**: SONORA signs its requests to slskd with a key. **Make a
   key** creates one and shows the lines to add to `slskd.yml`:

   ```yaml
   web:
     authentication:
       api_keys:
         sonora:
           key: <the key SONORA made>
           role: readwrite
   ```

   If `slskd.yml` already has a `web:` or `authentication:` section, add
   the lines inside it rather than a second one. Save the file, restart
   slskd, and choose **I restarted slskd: test**. A key you already have
   can be pasted instead. SONORA keeps the key in the macOS keychain or in
   Windows Credential Manager, never in its library or its logs.

   To let only this computer use the key, you can add
   `cidr: 127.0.0.1/32,::1/128` under `role`. Leave it out when slskd runs
   in Docker: requests then reach slskd from Docker's own address, and the
   key would be refused.
3. **Connection**: slskd's address, `http://localhost:5030` unless slskd
   runs elsewhere. **Test** shows slskd's version, the Soulseek user it is
   signed in as and how much it shares.
4. **Download folder**: the folder from `directories: downloads:` in
   `slskd.yml`. If slskd still uses its own default folder, SONORA offers
   it.

With slskd set up by SONORA, all of this is done; **Show the connection
details** shows it.

## Searching and downloading

On **Search**, type an artist, a title, a label or a catalogue number.
Soulseek matches every word against folder and file names, so fewer words
usually find more. slskd searches for about 15 seconds and hands over the
results when the search ends; until then SONORA shows how many people have
answered.

- Results are grouped by folder, one folder of one person's share per
  block. The folder from someone with a free upload slot and a fast
  connection comes first.
- The five bars beside each folder show how soon a download is likely to
  start: five lit for a free slot and a fast connection, one for a long
  queue.
- **Lossless** (the default) lists FLAC, WAV, AIFF, ALAC, WavPack and APE
  files; **All formats** adds MP3, AAC and the rest. Covers, cue sheets and
  other files that are not audio are never listed.
- Format, bit depth, sample rate, length and size are what the other
  person's client reports. They are not checked until the file arrives.
- **Download** on a file asks slskd for that file; **Download N files** on
  a folder asks for all of it, leaving out files already in your library.
  Files already in your library (same name and size) are marked **In
  library**.

**Downloads** lists slskd's downloads: queued (with the place in the other
person's queue), downloading, finished or failed. **Cancel** stops one;
**Clear finished** takes finished ones off the list in SONORA and in slskd,
and leaves the files where they are.

## Downloading a YouTube playlist

**YouTube playlist** on the Soulseek page turns a playlist into downloads:
paste the link of a public or unlisted playlist, and SONORA reads its track
list, looks for every track on Soulseek, picks the best file by your rules
and downloads it, one search at a time, while you do something else.

**Reading the list.** SONORA fetches the playlist's page the way a browser
does, with the playlist's id and nothing about you (no account, no
cookies but one that says no consent dialog is wanted), and follows the
page's own "load more" requests until the list is complete (up to 5,000
videos). No API key and no other program is needed. Private lists cannot
be read: Liked videos, Watch Later, Mixes and playlists set to private are
refused with a note; set a playlist to public or unlisted (on YouTube,
Edit → Visibility) and try again.

Each video's title is read as artist and title the way people write them:
`Artist - Title (Carl Craig Remix) [LABEL001]`, "PREMIERE:" at the front,
side markers (`A1.`, `B2 -`), track numbers, a year, a catalogue number in
brackets, "feat." and "ft.", hashtags, "(Official Video)", "- Preview" and
the like. The mix name is kept apart from the title, and for an "Artist -
Topic" channel the artist comes from the channel when the title has none.
What is plainly not a track is **left out** and listed under the tracks,
where a tick puts any of them back: Shorts, videos over 30 minutes (a mix
or a set), videos under two minutes with no artist – title structure, the
same track a second time, and deleted or private videos. Untick any track
you do not want.

**Rules.** Settings → **Playlist download rules** says which file to take
when a track is shared in several (the bar above the list shows the
choices made most often):

- Formats, best first: FLAC, AIFF, WAV and ALAC unless you change the order
  or turn one off; WavPack and APE off; lossy files (MP3, AAC, Ogg, Opus)
  off, or on from a bit rate up (320 kbps unless you change it). ALAC in
  `.m4a` files cannot be told from AAC by name, so they are taken and
  checked on arrival.
- Resolution: prefer 16-bit / 44.1 kHz, prefer the highest, or any; with an
  optional limit (reject above 24-bit, say, or above 96 kHz).
- Size: reject files far smaller than a lossless file of that length should
  be (a lossy file renamed), and files above a size you set.
- Who shares it: only people with a free upload slot, or a queue of at most
  so many downloads; a lowest reported upload speed.
- Matching: the share of the title's words the file name must have (60 %
  unless you change it); the artist must be in the file or folder name when
  the title names one; names that say remix, edit, live, radio or
  instrumental are left out unless the title says so too.
- Mode: **automatic** downloads the best file the moment a track is found;
  **review first** searches everything, shows the picks and downloads when
  you confirm. Downloads at a time, how long each search may run (15–25
  seconds), and whether to look again later for tracks not found.

A file must pass every rule. Among the files that do, the first format
wins, then the resolution preference, then a free upload slot, a short
queue and speed, then how well the name matches the title.

**Searching.** For each track SONORA tries up to five searches, stopping at
the first that finds a file passing the rules: the title (with its mix
name), the artist and the title, the artist and the title without the mix
name, the catalogue number, and the title with punctuation and apostrophes
stripped. Soulseek matches every word, so fewer words find more. The run
can be paused (searching stops after the current track; downloads on their
way go on), resumed and stopped, and it carries on where it was if you quit
SONORA in the middle. A run shows in the status bar from any page.

**Downloads.** The pick goes to slskd like any download, shows under
Downloads, joins your library when it finishes and is checked like every
arrival. If the file turns out to be made from a lossy source, damaged or
truncated, or lossy when the rules say lossless, or if the download fails
or moves nothing for 15 minutes (an hour in someone's queue), SONORA takes
the next best file, up to three tries per track, and notes it on the
track. A file that turned out wrong stays in your library, marked as such,
for you to decide on.

**The results.** One row per track: its status, the file chosen with its
format and reported quality, and a link to the video. Filter by status;
open a row for the other files that were found, why each was rejected,
**Use this file** to take another one instead (a download on its way is
cancelled), and the search that found it. **Retry** looks again for the
tracks that were not found or failed; with the rule on, SONORA looks again
by itself an hour after a run ends, up to three times, while it is open.

## Listening before you download

▶ beside a FLAC, WAV or AIFF file in the results plays it in SONORA's
player while it arrives. Nothing is downloaded into your library: a
preview is a way to decide whether you want the file.

**How it works.** Soulseek has no streaming: a file comes from the other
person's client in one transfer, from the start or from where an earlier
transfer of it stopped. slskd writes what arrives into a partial file in
its folder for unfinished downloads. For a preview, SONORA asks slskd for
the file, reads that partial file as it grows, and decodes it into a
cache of its own, which the player plays. Playback starts as soon as about
a second and a half has arrived, long before the whole file.

**What you see.** The player shows **Preview**, the file and who shares
it, and how it is going: asking that person, waiting in their queue (with
the place when they say), buffering, arriving as it plays, enough arrived
for now, the connection dropped and SONORA asks again, or why it cannot
play. The search result says Playing or Paused. You can pause, play on,
change the volume, close the player, and seek within what has arrived (the
seek bar shows how far that is). The length shown comes from the file's own
header once it arrives.

**It fetches only so far ahead.** A preview fetches at most **Read ahead**
(30 seconds, 1 minute or 2 minutes; 1 minute unless you change it, under
Previews on the Setup tab) beyond what is playing, or beyond what the
player has buffered if that is further. Further ahead, SONORA stops the
transfer; when playback gets closer, it asks slskd for the file again and
slskd carries on from where it stopped. So a file you skip after a minute
is not fetched whole. The other person sees each of these as a download
request. Whatever the player is waiting for is always fetched: the macOS
player buffers a while before it starts, and looks at the end of the file
first (SONORA answers that look at once, without fetching the rest).

**It keeps nothing.** When a preview stops (you close the player, play
something else, or quit SONORA), SONORA cancels the transfer, takes it off
slskd's list, deletes the partial file the preview made and deletes its
cache. If the whole file arrived during the preview, slskd saved it in a
hidden `.sonora-preview` folder in the download folder; SONORA deletes that
too. A partial file that was there before the preview (from a download that
stopped, say) is never deleted, and neither is any other file, even one
with the same name. A file that turns out not to be playable (not the
format its name says, say) is not fetched any further. If SONORA quits in
the middle of a preview, it cancels the transfer on the way out, and
deletes what is left the next time it opens. A preview is never added to
your library, never analysed, never listed under Downloads and never
shared: SONORA's folder watching skips `.sonora-preview`, and slskd shares
only the folders in its settings.

**Space.** The decoded audio of one preview takes at most **Space for
previews** (512 MB, 1 GB or 2 GB; 1 GB unless you change it), in SONORA's
cache folder. 1 GB holds about 100 minutes of CD-quality audio; a preview
longer than that stops where the space runs out, and says so.

**Download while listening.** **Download** in the player downloads the file
for real: slskd carries on from what the preview fetched, and the file
joins your library and is checked when it finishes, like any download. If
all of the file had arrived already, slskd fetches it again for the
download, and the preview's copy goes when the preview stops. If slskd
does not take the download, the preview goes on fetching by itself.

**A queue of previews.** The ＋ beside a result adds it to the preview
queue, shown above the results: **Play next**, take one out, **Clear**, or
**Play the queue**. With **Play the next one when a preview ends** (on
unless you turn it off), the next one starts when one ends; **Next
preview** in the player, or the media key, skips ahead. Nothing in the queue
is fetched until it plays.

**Formats.** FLAC, WAV and AIFF (uncompressed) can be decoded from their
first bytes, so they can be previewed. MP3, AAC, Ogg and Opus cannot be
previewed yet. M4A files keep what a player needs at their end, so they
cannot be played before they arrive whole. A WAV or AIFF that holds
compressed audio says so when its header arrives. The play button of a
file that cannot be previewed says why.

**What it needs.** Previews read slskd's partial files, so slskd must run
on this computer (with slskd on a NAS, the play buttons say so). slskd must
carry on from partial files, as it does unless `slskd.yml` says otherwise;
otherwise a preview plays only what arrives before its first pause, and
says it ends early.

A preview plays a file from someone else's share, so the same rules apply
to it as to a download (see the notice above).

## How downloads join the library

SONORA looks at the download folder every 20 seconds and takes a file once
it is finished:

- it is not a partial download: not in an `incomplete`, `Incomplete
  Downloads`, `temp` or `tmp` folder directly in the download folder, not
  in the `.sonora-preview` folder of previews, not named the way Nicotine+
  names partial files, and not ending in `.part`, `.tmp`, `.incomplete` or
  similar;
- its size and modification time are the same at two looks in a row, and
  it last changed at least 10 seconds ago;
- it can be opened for reading.

Each finished file is checked, hashed, compared for duplicates and
analysed like any other track. **Check now** on the Setup tab looks at
once.

On **Downloads**, a finished download shows what became of it: its score
and verdict once analysed, with the main issue, such as **Lossy source?**
for a lossless file made from a lossy one. Open the analysis for the
details.

To look at each file first, turn on **Review each download before it joins
the library**. Finished files then wait under **Waiting for you** on the
Downloads tab until you add them or keep them out.

Music that is already in the folder when you choose it was not downloaded
since, so SONORA leaves it alone: it is not added and not marked as coming
from Soulseek. The Setup tab says how many such files are not in your
library, with **Add them** if you want them after all.

SONORA only reads the download folder. It never moves, renames or deletes
a file there, except a preview's own `.sonora-preview` folder. A track you
remove from the library stays out, as it does for library folders. The
first time downloads arrive, SONORA reminds you that files from Soulseek
come from other people: it checks how a file sounds, not whether you may
have it.

## What SONORA keeps and sends

- **Sends to slskd**: the words you search for, the files you choose to
  download or preview, and (for a preview) how far playback got, with the
  API key. slskd sends the search words to Soulseek with your user name;
  other users see them.
- **Downloads from GitHub**: slskd's release file, only when you allow the
  setup or an update. The request carries nothing about you.
- **Fetches from YouTube**: a playlist's page and its "load more" answers,
  only when you paste a playlist link. The requests carry the playlist's
  id, a browser's User-Agent and a cookie that declines the consent dialog;
  no account, no history, nothing about your library. What SONORA keeps of
  a playlist (its videos, what was searched for and found, and which
  Soulseek users shared each file) stays in the library database until you
  remove the run.
- **Keeps**: the API key (in the system's keychain or Credential Manager),
  slskd's address, the download folder, and for each download that it came
  from Soulseek, its name on arrival and when it arrived. A track's
  analysis page shows this under **Came from**. While a preview plays, its
  decoded audio, in SONORA's cache folder, deleted when it stops.
- **slskd set up by SONORA**: your Soulseek password and slskd's web
  password are in `slskd.yml`, which only your user account can read
  (permissions 0600), and in its dated backups next to it, which only your
  account can read too. slskd reads its account from that file; SONORA
  keeps no other copy, never shows the Soulseek password and never writes
  it to its log. The launch agent holds no password. **Copy web password**
  puts slskd's web password on the clipboard when you ask, for its login
  page.
- **Does not keep**: anything about other users. User names appear in
  search results, downloads and previews while SONORA shows them, taken
  from slskd each time. When slskd reports a problem, SONORA's log notes
  only that a request failed, not slskd's message, which can name other
  users.
- Nothing goes anywhere but slskd (and GitHub for slskd itself). SONORA's
  requests go through the system's `curl`, straight to slskd's address.
  SONORA's own slskd has its web page and API on this computer only; like
  any Soulseek client, it takes connections from other Soulseek users on
  its Soulseek port.

## Soulseek's rules

Soulseek is run by Soulseek, LLC, and its [rules](https://www.slsknet.org/news/node/681)
and [terms of service](https://www.slsknet.org/news/node/682) apply to
everything slskd does with your account. Among them: share and download
only what you are allowed to; your account is yours alone; use is licensed
for personal, noncommercial, home use; and clients other than the official
ones are tolerated only on systems other than Windows, and only if they
implement the full range of Soulseek features. slskd is a complete client
(searching, downloading, sharing, chat and rooms), but whether it meets
that rule is for Soulseek to judge, not SONORA.

On **Windows**, use the official SoulseekQt client with the download folder
(see below), or run slskd on another computer (a Mac, a Linux machine or a
NAS) and enter its address in Setup. SONORA does not set slskd up on
Windows.

Sharing is up to you and set in slskd. SONORA's setup shares nothing unless
you choose a folder. Many Soulseek users give priority to, or only upload
to, people who share.

## Another Soulseek client

SONORA's folder watching works with any client. With SoulseekQt or
Nicotine+, choose the folder where it saves finished downloads on the
Setup tab; finished files join the library as described above. Searching,
previews and the downloads list need slskd.

## Troubleshooting

**The setup says "Checking the download did not work"**: what arrived from
GitHub is not the slskd release SONORA expects, so SONORA did not use it
(and deleted its own download). Try again; if it happens again, tell us:
the release may have changed.

**The setup says "Starting slskd did not work"**: the message says why.
Most often another slskd holds its ports: **Stop it**, then **Try again**.

**The setup or the Setup tab says Soulseek did not accept the user name or
password**: type the password again (the one you use in the Soulseek app).
A user name that is new to Soulseek is created at the first sign-in; one
that exists needs its own password.

**"SONORA could not reach slskd"**: slskd is not running, or it listens
somewhere else. With slskd set up by SONORA, **Start** on the Setup tab
starts it. Otherwise open its web page to check, and compare the address
with the one in Setup.

**"slskd did not accept the API key"**: with slskd set up by SONORA,
**Repair** fixes it. Otherwise the key in `slskd.yml` differs from the one
SONORA saved, the `cidr` line does not include this computer, or slskd was
not restarted after the change.

**"slskd refused the request"**: a slskd running as a relay agent does not
search or download; enter the address of the main slskd. Otherwise give the
key `role: readwrite`.

**"SONORA could not check slskd's https certificate"**: slskd makes a new
certificate each time it starts, which no system trusts. Use `http://` for
slskd on this computer, or give slskd a certificate your system trusts (a
reverse proxy with a certificate from Let's Encrypt, say).

**"slskd is not connected to Soulseek"**: the Setup tab says why when slskd
was set up by SONORA. Otherwise check the user name and password in
`slskd.yml` and slskd's web page. The Soulseek server is sometimes down
for a while.

**A preview stays at "Asking…" or "Waiting in … queue"**: the other person
has no free upload slot, or their client does not answer. Try a result
from someone with a free slot (five bars).

**A preview ends early**: the other person's client stopped the transfer,
or slskd starts partial files over instead of carrying on from them. The
preview plays what arrived.

**A finished download is not added**: check that the download folder in
Setup is slskd's (`directories: downloads:`), and look at the Setup tab for
a message about the folder. Files that keep changing, such as one still
being written by another program, are taken once they settle.

**A download fails with "The other user turned the request down"**: the
file is no longer shared, or that person only uploads to some users. Try
another folder from the results.

**macOS asks whether SONORA may use the keychain**: after an update that
changed how SONORA is signed, macOS asks once; choose **Always Allow**.

## Limits

- SONORA sets slskd up and keeps it running on macOS only. On Windows and
  Linux, set slskd up yourself (Windows: see
  [Soulseek's rules](#soulseeks-rules)).
- SONORA's slskd lives in SONORA's own folder in Application Support, not
  in a folder you choose: macOS protects Desktop, Documents and Downloads,
  and slskd running in the background there would be stopped by privacy
  prompts with no one to answer them.
- SONORA follows slskd's API as of slskd 0.26. Much older versions may
  answer differently.
- SONORA lists the best 150 folders of a search (up to 3,000 files).
- ALAC in `.m4a` files cannot be told from AAC by name, so `.m4a` files
  show under **All formats**.
- Formats and sizes in results are reported by other people's clients and
  can be wrong; SONORA's analysis of the downloaded file is what counts.
- Previews play FLAC, WAV and AIFF only, need slskd on the same computer,
  and seek only within what has arrived. A preview is fetched from the
  start of the file: Soulseek has no way to ask for the middle of a file.
- On Windows, slskd keeps its partial file locked while it writes, so a
  preview fetches a piece, stops the transfer to read it, and asks again.
  This has been tested with a stand-in for slskd, not with slskd on
  Windows.
- SONORA does not bring slskd inside its installer; the setup downloads
  slskd's own release. It does not set up slskd's chat or other settings.
- A YouTube playlist is read from its public page; if YouTube changes the
  page, reading may stop working until SONORA is updated. Titles are read
  as people write them, so an unusual title can come out as the wrong
  artist or title: untick it, or change what was found afterwards.
