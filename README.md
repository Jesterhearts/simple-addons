# simple-addons

Install World of Warcraft addons from a folder or a zip file.

Run it with no arguments for a window you can drag addons onto. Give it paths
instead and it installs them and prints what it did.

## The window

```
cargo run
```

Drop addon folders or zip files anywhere on the window and they install. The
pickers at the top choose the game folder and the game version; buttons in the
drop area do the same job for anyone who would rather not drag anything.

The window remembers the game folder and version you chose, so it opens on
them next time rather than going back to retail. A `--flavor` on the command
line is an instruction for that run and wins over what was remembered. If the
folder or version is gone by the next run, the choice is dropped rather than
restored.

## The command line

```
simple-addons [OPTIONS] <SOURCE>...
```

| Option | What it does |
| --- | --- |
| `-w`, `--wow-dir <PATH>` | The World of Warcraft directory. Found automatically when left out. Also read from `SIMPLE_ADDONS_WOW_DIR`. |
| `-f`, `--flavor <NAME>` | Game version to install into, such as `retail` or `classic_era`. Defaults to retail. |
| `-n`, `--dry-run` | Report what would be installed without writing anything. |
| `--list` | List the World of Warcraft directories that were found, then stop. |
| `--gui` | Open the window even when sources are given. The window installs them as it opens. |

```console
$ simple-addons --list
/home/you/Games/battlenet/drive_c/Program Files (x86)/World of Warcraft
  Retail         118 addons installed
  Classic Beta   no addon folder yet

$ simple-addons ~/Downloads/WeakAuras-5.19.1.zip ~/Downloads/Details
Installing to /home/you/Games/.../World of Warcraft (Retail)
  installed  WeakAuras         5.19.1
  installed  WeakAurasOptions  5.19.1
  updated    Details           12.3.4
```

The exit status is non-zero if any addon failed, so this works in a script.

## Starting it from the launcher

```
scripts/install-desktop-entry.sh
```

This runs `cargo install --path .` and writes a desktop entry naming the
program cargo installed, so Simple Addons appears in the application
launcher. The entry opens the window, and files dropped on its icon are
installed as the window opens.

The entry and its icon go under `~/.local` by default; `--prefix DIR` puts
them somewhere else. `--no-install` writes only the entry, for when cargo
install has already been run. `--uninstall` undoes the lot. Rerun the script
after changing the code to install the new version.

## What it handles

Downloads rarely arrive in the shape the game wants, which is
`Interface/AddOns/<Name>` holding `<Name>.toc`. These all install correctly:

- a bare addon folder;
- a zip file holding the addon folder;
- a zip file that wraps the addon in a release folder, such as
  `WeakAuras-5.19.1/WeakAuras/`, which is what GitHub releases look like;
- a bundle holding several addon folders side by side;
- a zip file with the `.toc` at the top level and no folder at all;
- an addon whose toc files carry game version suffixes, such as
  `MyAddon_Vanilla.toc`.

Shared libraries an addon embeds under `Libs/` stay inside it rather than
being installed separately, and files beside the addon, such as a `README`,
are left out.

Updating an addon replaces its folder, so files dropped by a new version do
not linger. Saved variables live in `WTF/`, not in the addon folder, so
settings survive an update.

## Where it looks for the game

A World of Warcraft directory is one holding a game version folder such as
`_retail_`. The search covers Wine prefixes made by Lutris, Steam, Bottles and
Heroic, the plain `~/.wine` prefix, and install folders outside any prefix. If
yours is somewhere else, pass `--wow-dir` or use the picker in the window.

`--wow-dir` accepts the install root, a game version folder, an `Interface`
folder or an `AddOns` folder, so whatever you have at hand will do.

## Development

```
cargo test
cargo clippy --all-targets
cargo +nightly fmt
```

The desktop entry and the icon are data, kept in `resources/` and installed
by the script rather than written by it.

## License

This software is released into the public domain under the Unlicense. See
[UNLICENSE](UNLICENSE) for the full text.
