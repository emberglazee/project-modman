# Game audio (`.ogg`)

Project Wingman keeps its audio inside the pak as Ogg Vorbis: menu clicks,
engine loops, radio chatter, mission music. modman can list those assets and
write each one out as a playable `.ogg`, without transcoding anything.

## Why `.ogg` and not `.wav`

The game already stores the compressed stream, so extraction is a matter of
finding where it starts and where it ends and copying those bytes. A `.wav`
would mean decoding the Vorbis and re-encoding as PCM: bigger files, a lossy
decode, and a decoder dependency, all to arrive at something the game never had.

## Commands

```bash
# What is in there? (no path: the game's own pak, auto-detected)
modman audio list
modman audio list --filter Dialog
modman audio list <pak-or-dir...>

# Write the files. The tree mirrors the asset paths.
modman audio extract -o pw-audio --filter "Dialog/C_M6"
modman audio extract -o pw-audio --manifest audio.csv
modman audio extract ~/.steam/steam/steamapps/common/"Project Wingman"/ReplaceKingsWithArchange_P.pak -o mod-audio
```

`--filter` is a case-insensitive substring match on the asset path, which is
enough to pull one mission's dialogue (`Dialog/C_M6/`), one aircraft's engine
loops, or a single music folder.

`--manifest` writes a CSV of what was written: `asset,output,bytes,streamed`.

Only `SoundWave` assets carry audio. `SoundCue` nodes are graphs that point at
SoundWaves, and `SoundAttenuation` and `SoundMix` carry no payload at all, so
they are counted and skipped.

## Streamed tracks, and the truncated-file trap

Most sounds sit entirely inside their `.uexp`. The long music tracks do not:
the pak keeps a 256 KB prefix inline and the rest in a sibling `.ubulk`, and the
split lands in the middle of an Ogg page.

A tool that reads only the `.uexp` produces a file that still opens and plays
for around 20 seconds before stopping. That is the failure to watch for. It is
also what made the `ReplaceKingsWithArchange` mod look un-extractable: its
replacement tracks are streamed, so the payload is not in the `.uexp` alone, and
the bulk file begins mid-page with no beginning-of-stream page of its own.

`modman audio extract` handles this by taking the inline chunk and appending the
bulk, then trimming at the Ogg end-of-stream page. Padding after that page is
dropped, which matters: one of the Kings tracks carries 79 KB of it.

## Checking a result

Durations from the extracted files match the `Duration` property the engine
stores on the same asset, so a wrong cut shows up immediately:

| asset | engine `Duration` | extracted |
|---|---|---|
| `C_M22/PW_-_Final_Mission_Full` | 424.615 s | 424.615 s |
| `C_M22/PW_-_Final_Mission_Full_-_Loop` | 380.308 s | 380.308 s |
| `C_M22/PW_-_CrimSomberV3` | 162.000 s | 162.000 s |
| the same three in the Archange mod | 248.611 / 154.681 / 162.000 s | same |

`ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 <file>`
prints the duration, and `ffmpeg -v error -i <file> -f null -` reports any
decode error.

## What this does not do

- **One track sits behind UE bulk compression.** The Frontline-59 track
  `MagadanFront/.../Music/M05/PW_MF_M5_LOOP` is stored with UE's own bulk
  compression (bulk flags `0x06`) instead of a raw Vorbis stream, so `extract`
  names it and skips it. The other 5,118 sound assets in the pak come out and
  decode cleanly.
- **No packing back in.** Replacing a track in a mod means building a SoundWave
  asset, which is a different job from extracting one.
- **No conversion.** The files are the game's Vorbis streams; use any audio tool
  if you want `.wav`, `.mp3` or normalisation.
- **No commentary on loudness or looping.** Loop points live in the asset, not in
  the stream, so an extracted music loop will play once rather than loop.
