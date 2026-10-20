# Soulseek

SONORA can search Soulseek for lossless music, download what you choose,
and add each finished file to your library with its quality checked, so a
"FLAC" made from an MP3 shows up as soon as it arrives.

SONORA never connects to Soulseek itself. It works through **slskd**, a free
and complete Soulseek client that you install and run with your own
Soulseek account. slskd does everything on the network: searching,
downloading and the sharing you set up in it. SONORA sends slskd
instructions (search for these words, download these files) and picks up
the finished files from slskd's download folder.

- [Before you start](#before-you-start)
- [Setting up slskd](#setting-up-slskd)
- [Connecting SONORA](#connecting-sonora)
- [Searching and downloading](#searching-and-downloading)
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
> download, copy and use is lawful where you live and allowed by that
> network's terms. SONORA does not own, host or license music found on other
> networks, and it never shares your files.

Other Soulseek users see what you search for, along with your Soulseek user
name. SONORA is not affiliated with Soulseek, LLC or with slskd.

## Setting up slskd

You need slskd running on your computer (or on another computer you can
reach, such as a NAS) and signed in to Soulseek. If you have never used
Soulseek, the first sign-in with a new user name creates the account; there
is no way to reset a forgotten password, so keep it somewhere safe.

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

Open **Soulseek** in the sidebar (⌘8 on a Mac, Ctrl+8 on Windows), accept
the notice, and go through **Setup**:

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
   Windows Credential Manager, never in its library or its logs. On a Mac
   it is saved so that every build of SONORA can read it without macOS
   asking first (what Keychain Access calls "Allow all applications to
   access this item"); like `slskd.yml`, which holds the same key as plain
   text, it is readable by programs running under your account.

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
- Format, bit depth, sample rate and size are what the other person's
  client reports. They are not checked until the file arrives.
- **Download** on a file asks slskd for that file; **Download N files** on
  a folder asks for all of it, leaving out files already in your library.
  Files already in your library (same name and size) are marked **In
  library**.

**Downloads** lists slskd's downloads: queued (with the place in the other
person's queue), downloading, finished or failed. **Cancel** stops one;
**Clear finished** takes finished ones off the list in SONORA and in slskd,
and leaves the files where they are.

## How downloads join the library

SONORA looks at the download folder every 20 seconds and takes a file once
it is finished:

- it is not a partial download: not in an `incomplete`, `Incomplete
  Downloads`, `temp` or `tmp` folder directly in the download folder, not
  named the way Nicotine+ names partial files, and not ending in `.part`,
  `.tmp`, `.incomplete` or similar;
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
a file there. A track you remove from the library stays out, as it does for
library folders. The first time downloads arrive, SONORA reminds you that
files from Soulseek come from other people: it checks how a file sounds,
not whether you may have it.

## What SONORA keeps and sends

- **Sends to slskd**: the words you search for and the files you choose to
  download, with the API key. slskd sends the search words to Soulseek with
  your user name; other users see them.
- **Keeps**: the API key (in the system's keychain or Credential Manager),
  slskd's address, the download folder, and for each download that it came
  from Soulseek, its name on arrival and when it arrived. A track's
  analysis page shows this under **Came from**.
- **Does not keep**: anything about other users. User names appear in
  search results and downloads while SONORA shows them, taken from slskd
  each time. When slskd reports a problem, SONORA's log notes only that a
  request failed, not slskd's message, which can name other users.
- Nothing goes anywhere but slskd. SONORA's requests go through the
  system's `curl`, straight to slskd's address.

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
NAS) and enter its address in Setup.

Sharing is up to you and set in slskd. Many Soulseek users give priority
to, or only upload to, people who share.

## Another Soulseek client

SONORA's folder watching works with any client. With SoulseekQt or
Nicotine+, choose the folder where it saves finished downloads on the
Setup tab; finished files join the library as described above. Searching
and the downloads list need slskd.

## Troubleshooting

**"SONORA could not reach slskd"**: slskd is not running, or it listens
somewhere else. Open its web page to check, and compare the address with
the one in Setup.

**"slskd did not accept the API key"**: the key in `slskd.yml` differs from
the one SONORA saved, the `cidr` line does not include this computer, or
slskd was not restarted after the change.

**"slskd refused the request"**: a slskd running as a relay agent does not
search or download; enter the address of the main slskd. Otherwise give the
key `role: readwrite`.

**"SONORA could not check slskd's https certificate"**: slskd makes a new
certificate each time it starts, which no system trusts. Use `http://` for
slskd on this computer, or give slskd a certificate your system trusts (a
reverse proxy with a certificate from Let's Encrypt, say).

**"slskd is not connected to Soulseek"**: check the user name and password
in `slskd.yml` and slskd's web page. The Soulseek server is sometimes down
for a while.

**A finished download is not added**: check that the download folder in
Setup is slskd's (`directories: downloads:`), and look at the Setup tab for
a message about the folder. Files that keep changing, such as one still
being written by another program, are taken once they settle.

**A download fails with "The other user turned the request down"**: the
file is no longer shared, or that person only uploads to some users. Try
another folder from the results.

**macOS asks whether SONORA may use the keychain**: a key saved by SONORA
0.3.0 or earlier is tied to the exact build that saved it, so the first
launch of a newer version asks once; choose **Always Allow**. SONORA then
saves the key again so that no later build asks. If you declined, or the
dialog went unnoticed and the Setup tab says the key could not be read,
choose **Try again** (the dialog returns) or **Replace** the key with the
one in `slskd.yml`.

## Limits

- SONORA follows slskd's API as of slskd 0.26. Much older versions may
  answer differently.
- SONORA lists the best 150 folders of a search (up to 3,000 files).
- ALAC in `.m4a` files cannot be told from AAC by name, so `.m4a` files
  show under **All formats**.
- Formats and sizes in results are reported by other people's clients and
  can be wrong; SONORA's analysis of the downloaded file is what counts.
- SONORA does not bring slskd with it, and does not set up slskd's sharing,
  chat or other settings.
