# simple-addons

Install World of Warcraft addons from a folder or a zip file, and remove them again.

Run it with no arguments for a window you can drag addons onto. Give it paths instead and it
installs them and prints what it did.

## The window

```
cargo run
```

Drop addon folders or zip files anywhere on the window and they install. The pickers at the top
choose the game folder and the game version; buttons in the drop area do the same job for anyone who
would rather not drag anything.

The Installed addons tab lists what the game version has, with a filter for finding one name among
many. Tick the addons to remove and choose Remove. Nothing is removed until a dialog has named every
addon it will remove; see [Removing addons](#removing-addons) for what it checks first.

The window remembers the game folder and version you chose, so it opens on them next time rather
than going back to retail. A `--flavor` on the command line is an instruction for that run and wins
over what was remembered. If the folder or version is gone by the next run, the choice is dropped
rather than restored.

### Drag and drop and Wayland

The window opens on X11 whenever an X display is there to open on, including the XWayland that a
Wayland session normally runs. This is not a preference: the Wayland backend the window is built on
reports no dropped files at all, so a window opened on Wayland can never see a drop.

`SIMPLE_ADDONS_BACKEND=wayland` asks for a native Wayland window instead, at the cost of drag and
drop. The drop area says so and points at the buttons when that is where the window ended up, which
is also what happens on a Wayland session with no XWayland. `SIMPLE_ADDONS_BACKEND=x11` asks for X11
explicitly; any other value is ignored.

## The command line

```
simple-addons [OPTIONS] <SOURCE>...
simple-addons [OPTIONS] --remove <ADDON>...
```

| Option                      | What it does                                                                                                                                                          |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `-w`, `--wow-dir <PATH>`    | The World of Warcraft directory. Found automatically when left out. Also read from `SIMPLE_ADDONS_WOW_DIR`.                                                           |
| `-f`, `--flavor <NAME>`     | Game version to install into or remove from, such as `retail` or `classic_era`. Defaults to retail.                                                                   |
| `-n`, `--dry-run`           | Report what would be installed or removed without changing anything.                                                                                                  |
| `-r`, `--remove <ADDON>...` | Remove these addons instead of installing anything. Give names as the folders are named; capital letters only matter between two folders that differ in nothing else. |
| `--list`                    | List the World of Warcraft directories that were found, then stop.                                                                                                    |
| `--list-addons`             | List the addons installed in the game version, then stop.                                                                                                             |
| `--gui`                     | Open the window even when sources are given. The window installs them as it opens.                                                                                    |

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

$ simple-addons --remove weakauras
Removing from /home/you/Games/.../World of Warcraft (Retail)
  removed  WeakAuras  5.19.1
These addons need an addon that was removed, so the game will not load them:
  WeakAurasArchive  5.19.1
  WeakAurasOptions  5.19.1
To remove them as well, add their names to --remove.
```

The exit status is non-zero if any addon failed, or if a name given to `--remove` is not installed,
so this works in a script.

## Starting it from the launcher

```
scripts/install-desktop-entry.sh
```

This runs `cargo install --path .` and writes a desktop entry naming the program cargo installed, so
Simple Addons appears in the application launcher. The entry opens the window, and files dropped on
its icon are installed as the window opens.

The entry and its icon go under `~/.local` by default; `--prefix DIR` puts them somewhere else.
`--no-install` writes only the entry, for when cargo install has already been run. `--uninstall`
undoes the lot. Rerun the script after changing the code to install the new version.

## What it handles

Downloads rarely arrive in the shape the game wants, which is `Interface/AddOns/<Name>` holding
`<Name>.toc`. These all install correctly:

- a bare addon folder;
- a zip file holding the addon folder;
- a zip file that wraps the addon in a release folder, such as `WeakAuras-5.19.1/WeakAuras/`, which
  is what GitHub releases look like;
- a bundle holding several addon folders side by side;
- a zip file with the `.toc` at the top level and no folder at all;
- an addon whose toc files carry game version suffixes, such as `MyAddon_Vanilla.toc`.

Shared libraries an addon embeds under `Libs/` stay inside it rather than being installed
separately, and files beside the addon, such as a `README`, are left out.

Updating an addon replaces its folder, so files dropped by a new version do not linger. Saved
variables live in `WTF/`, not in the addon folder, so settings survive an update. If some files of
the old copy cannot be deleted, the update still goes ahead; see [Removing addons](#removing-addons)
for where those files go.

## Removing addons

Removing an addon deletes its folder from `Interface/AddOns`. The folder is moved out of `AddOns`
first and deleted after, so the game never finds part of an addon where a whole one used to be. The
settings the game saved for an addon live in `WTF/`, not in its folder, so they are kept, and
installing the addon again brings them back.

Some addons need another one to load. WeakAurasOptions, for example, names WeakAuras in its `.toc`
file as a required dependency. Removing WeakAuras leaves WeakAurasOptions installed but unused, so
removing an addon also looks for the installed addons that need it:

- the window names them in its dialog, with a tick box to remove them too that starts ticked;
- the command line names them after its report and removes only what it was asked to.

An addon folder that is a symbolic link, such as a link to a working copy of an addon you are
writing, is removed as a link. The folder it leads to is not touched.

The window's list and `--list-addons` also show folders the game does not load, because no `.toc`
file in them is named after the folder. These are usually left over from an addon unpacked by hand,
and removing them works the same way.

Files that cannot be deleted, such as a read-only folder an addon brought with it, are moved into
`Interface/.simple-addons-left-over` instead, and the report says where. The addon is still removed,
since the game no longer finds it, but the command line exits non-zero so that a script notices the
files to delete by hand. An update that did not finish, for example because the program was killed
in the middle of it, is undone by the next run, which puts the old copy of the addon back. If
something has taken its place by then, the old copy goes to the left-over folder instead. It is
never deleted.

Only one copy of the program installs or removes addons in a game version at a time. Another copy
started meanwhile, for example by dropping files on the launcher icon, reports that it is busy
instead of getting in the way. Some network filesystems cannot lock a folder; there, runs go ahead
without this check.

## Where it looks for the game

A World of Warcraft directory is one holding a game version folder such as `_retail_`. The search
covers Wine prefixes made by Lutris, Steam, Bottles and Heroic, the plain `~/.wine` prefix, and
install folders outside any prefix. If yours is somewhere else, pass `--wow-dir` or use the picker
in the window.

`--wow-dir` accepts the install root, a game version folder, an `Interface` folder or an `AddOns`
folder, so whatever you have at hand will do. A path inside a game version folder also chooses that
game version, unless `--flavor` names another. A path inside the World of Warcraft directory but not
inside a game version folder found there is refused unless `--flavor` names one, rather than quietly
using retail. A game version folder may be a symbolic link, for example to another disk.

When the search finds several World of Warcraft directories, installing uses the first, and the
report names it. Removing cannot be undone, so it refuses to guess and asks for `--wow-dir`.

## Development

```
cargo test
cargo clippy --all-targets
cargo +nightly fmt
```

The desktop entry and the icon are data, kept in `resources/` and installed by the script rather
than written by it.

## License

This software is released into the public domain under the Unlicense. See [UNLICENSE](UNLICENSE) for
the full text.
