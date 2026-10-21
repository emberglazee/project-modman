# Merging mods in the browser

This page merges Project Wingman mods that fight over the same files, with no
installation and no command line. Everything happens inside your browser tab:
nothing is uploaded, and the page works offline.

If you would rather use the desktop program, read
[the command line walkthrough](command-line-walkthrough.md) instead. It is the
same engine with no memory limit.

## What you need

- The mods you want to use together, as `.pak` files.
- `pakchunk0-WindowsNoEditor.pak` from your own game install. It lives in
  `Project Wingman/ProjectWingman/Content/Paks/` and is about 16 GB. You do not
  upload it; the page reads a few megabytes from it in place.

## Step 1: unpack the downloads

Mod sites hand you `.zip`, `.7z` or `.rar` archives. **Extract them first.**
A `.zip` renamed to `.pak` is not a mod and the merger will say so.

Inside the extracted folder you want files ending in `_P.pak`.

## Step 2: add the mod paks

Drag them onto the first box, or use **Choose files…**. The list shows what you
added and how big each one is.

Order matters only for conflicts: later filenames win. The report tells you
exactly which field was set by which mod, so you do not have to guess.

## Step 3: point at your game pak

Use **Choose game pak…** and pick `pakchunk0-WindowsNoEditor.pak` from your own
install. The page only reads the parts it needs.

## Step 4: merge and install

Press **Merge mods**. When it finishes you get a download button for
`SicarioCombine_P.pak`.

To install:

1. Put that file in `Project Wingman/ProjectWingman/Content/Paks/~mods/`.
2. Move the mod paks you just merged **out** of `~mods`. Keeping them installed
   alongside the merged file means the game loads the same edits twice.
3. Start the game.

## When it says it ran out of memory

Browsers cap how much memory a page may use. A dozen aircraft addons at 40 to
50 MB each can reach that cap, especially with other tabs open.

In order of effort:

1. Close other tabs, reload, and try again.
2. Merge in batches: merge three mods, then add the result to the next round
   along with three more. The merged file behaves like any other mod.
3. Use the desktop build. It has no such limit and takes one command. See
   [the command line walkthrough](command-line-walkthrough.md).

The error box tells you which of these applies, and the diagnostics text inside
it can be copied into a bug report. That text is what makes a report fixable:
it lists the mods, their sizes, the game pak, and where the merge stopped.

## What this page cannot do

- It cannot merge mods that ship Sicario metadata (`.dtm` files or presets
  inside a pak). Those need `modman build`, which is in the desktop build.
- It cannot edit `.locres` translations. That is `modman locres`.
- It does not read or write anything outside your browser tab.
