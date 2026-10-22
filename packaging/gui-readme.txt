Project Modman: desktop app (GUI) for Project Wingman

Requires a 64-bit Linux desktop with OpenGL 3.3 (X11 or Wayland).

Run it
  chmod +x modman-gui
  ./modman-gui

The game is found automatically; if it is somewhere unusual, use
"Choose game folder...".

What it does
  Drop .rar, .7z, .zip or .pak mod files on the window. It extracts them
  in-process, merges the ones that edit the same files, shows the conflict
  report, and installs the result into the game's ~mods folder with a
  timestamped backup of whatever it replaced.

Tabs
  Merge mods      the merge, with drag and drop
  Build presets   build a Sicario preset set into one mod pak
  Game audio      list the game's sound assets, extract them as .ogg
  Translations    extract a .locres to CSV, patch it back from a CSV
  Pak files       inspect, unpack and repack .pak files
  Scan & inspect  list installed Sicario components; inspect a patch file

Notes
  - Every tab calls the same code the modman command line runs, so the output
    is identical; the window adds dialogs, archive extraction and the install
    step, nothing else.
  - Unsigned binary. Your file manager may need permission to run it.
  - Same binary as the GitHub release asset modman-gui-linux-<arch>, so
      gh attestation verify modman-gui --repo emberglazee/project-modman
    proves where it was built.
