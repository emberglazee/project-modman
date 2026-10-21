# Using the command line, step by step

This guide assumes you have never used a command line. It is written for the
common case: you have two or more Project Wingman mods that cannot be installed
together because they change the same files, and you want one merged pak.

If you would rather not touch a terminal at all, use
[the browser page](https://emberglazee.github.io/project-modman/) instead. It
does the same merge. The command line is faster, has no memory limit, and can
also build Sicario mods.

## 1. Get the program

Download the file for your system from the
[releases page](https://github.com/emberglazee/project-modman/releases/latest):

| System | File |
|---|---|
| Windows (normal PC) | `modman-win-amd64.exe` |
| Windows on ARM | `modman-win-arm64.exe` |
| Linux | `modman-linux-amd64` |
| macOS (Apple Silicon) | `modman-macos-arm64` |
| macOS (Intel) | `modman-macos-amd64` |

Make a folder for it, for example `C:\modman`, and put the file there. Rename it
to `modman.exe` so the commands below match. On Linux or macOS, also run
`chmod +x modman` once.

Windows may warn that the file is unsigned. That is normal for small open source
tools; the build comes from this repository's CI. **More info → Run anyway** if
you trust the source.

## 2. Open a terminal in that folder

On Windows:

1. Open the `C:\modman` folder in File Explorer.
2. Click the address bar, type `powershell`, and press Enter.

A window opens with a prompt ending in `C:\modman>`. Everything below is typed
there, then Enter.

On Linux or macOS, open your terminal and `cd` to the folder.

## 3. Find your two folders

**Your game folder** is the one containing `ProjectWingman`. Usually:

```
C:\Program Files (x86)\Steam\steamapps\common\Project Wingman
```

**Your mods folder** is inside it:

```
C:\Program Files (x86)\Steam\steamapps\common\Project Wingman\ProjectWingman\Content\Paks\~mods
```

You can let the program find the game itself. You only need the path if it
guesses wrong, or if the game is somewhere unusual.

## 4. The one command you need

This merges every pak in your mods folder into a single file:

```powershell
.\modman.exe combine "$env:PROGRAMFILES(X86)\Steam\steamapps\common\Project Wingman\ProjectWingman\Content\Paks\~mods\*.pak" --output C:\modman\out
```

What each part means:

| Part | Meaning |
|---|---|
| `.\modman.exe` | run the program in this folder. The `.\` is required on Windows. |
| `combine` | the job: merge mods that fight over the same files |
| `"…\*.pak"` | every pak in your mods folder. Quotes matter because the path has spaces. |
| `--output C:\modman\out` | where to put the result. The folder is created if needed. |

If your game is installed somewhere else, add its folder:

```powershell
.\modman.exe combine "D:\Games\Project Wingman\ProjectWingman\Content\Paks\~mods\*.pak" --install-path "D:\Games\Project Wingman" --output C:\modman\out
```

Linux and macOS, same idea:

```bash
./modman combine ~/.steam/steam/steamapps/common/"Project Wingman"/ProjectWingman/Content/Paks/~mods/*.pak --output ./out
```

## 5. Read what it says

A good run looks like this:

```
Merged 4 datatable(s), 61 file(s) total.
Wrote C:\modman\out\SicarioCombine_P.pak
```

It also lists, for each conflict, which mod won and what the value was. That
list is the point of the tool: two mods that both edit the same aircraft row are
merged field by field instead of one silently replacing the other.

If it prints warnings about files it could not combine, those files are kept
from the last mod in the order. The report names them.

## 6. Install the result

1. Copy `SicarioCombine_P.pak` into your `~mods` folder.
2. Move the original mod paks **out** of `~mods`. Put them somewhere else, for
   example a `mods-backup` folder next to the game. If you leave them in place
   the game loads the same edits twice.
3. Start the game. Paks are read at launch, so a restart is required.

## If your mods are Sicario mods

Mods that ship `.dtm` files, or a `sicario` folder inside the pak, use a
different command. Point it at the files instead of a folder of paks:

```powershell
.\modman.exe build "C:\modman\mods\*.dtm" --output C:\modman\out
```

`build` understands patch metadata and mod parameters. `combine` does not need
any metadata and works on plain override paks, which is the case for most mods
released after 2024.

Leaving out `--output` prints a preview and writes nothing. That is a safe way
to see what a mod set would do.

## When something goes wrong

| Message | What it means | Fix |
|---|---|---|
| `'modman.exe' is not recognized` | you are not in the folder, or the `.\` is missing | `cd C:\modman` first, keep the `.\` |
| `The system cannot find the path specified` | a path is wrong or a folder name has a typo | copy the path from Explorer's address bar, keep the quotes |
| `Could not locate Project Wingman` | auto-detection failed | add `--install-path "…\Project Wingman"` |
| `Invalid pak` or `magic` | the file is not a pak (often a renamed zip) | extract the archive and use the `.pak` inside |
| It merges but nothing changes in game | the originals are still in `~mods`, or the game was not restarted | move them out, restart |
| Windows Defender blocks the download | unsigned binary | allow it, or build from source |

If a run fails in a way this table does not cover, run the same command again
and copy the whole output into a bug report. The last lines name the file and
the step that failed.

## Doing this repeatedly

Once it works, put the command in a `.bat` file next to `modman.exe` so you can
double-click it instead of typing:

```bat
@echo off
modman.exe combine "%~dp0..\..\Steam\steamapps\common\Project Wingman\ProjectWingman\Content\Paks\~mods\*.pak" --output "%~dp0out"
pause
```

The `pause` keeps the window open so you can read the result.
