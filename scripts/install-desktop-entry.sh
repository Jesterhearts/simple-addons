#!/usr/bin/env bash
#
# Creates a desktop entry so that Simple Addons can be started from the
# application launcher.
#
# The program itself is installed by `cargo install --path .`, and the entry
# names the path cargo put it at. Keeping cargo in charge of the program means
# there is one copy to update, by rerunning this script or cargo install
# directly.

set -euo pipefail

readonly APP_ID="simple-addons"

REPO="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
readonly REPO

usage() {
	cat <<'USAGE'
Usage: scripts/install-desktop-entry.sh [OPTIONS]

Runs `cargo install --path .` and writes a desktop entry naming the program
cargo installed, so that Simple Addons can be started from the application
launcher.

Options:
      --uninstall    Remove the entry and the icon, and cargo uninstall the
                     program.
      --no-install   Leave the program alone and only write the entry, for
                     when cargo install has already been run.
      --prefix DIR   Where the entry and the icon go. Defaults to ~/.local,
                     which is where a launcher looks for one user's own
                     applications. Pass the same prefix when uninstalling.
  -h, --help         Show this message.

The program goes wherever cargo installs it, which is CARGO_INSTALL_ROOT or
CARGO_HOME or ~/.cargo, and not under --prefix.
USAGE
}

main() {
	local prefix="${HOME}/.local"
	local run_cargo=yes
	local action=install

	while [ $# -gt 0 ]; do
		case "$1" in
		--uninstall) action=uninstall ;;
		--no-install) run_cargo=no ;;
		--prefix)
			[ $# -ge 2 ] || die "--prefix needs a directory"
			prefix="$2"
			shift
			;;
		--prefix=*) prefix="${1#--prefix=}" ;;
		-h | --help)
			usage
			return 0
			;;
		*) die "unknown option: $1 (try --help)" ;;
		esac
		shift
	done

	local entry="${prefix}/share/applications/${APP_ID}.desktop"
	local icon="${prefix}/share/icons/hicolor/scalable/apps/${APP_ID}.svg"

	if [ "${action}" = uninstall ]; then
		uninstall "${run_cargo}" "${entry}" "${icon}"
	else
		install_all "${run_cargo}" "${entry}" "${icon}"
	fi
	refresh_caches "${prefix}"
}

install_all() {
	local run_cargo="$1" entry="$2" icon="$3"

	[ "${run_cargo}" = no ] || cargo install --path "${REPO}"

	local binary
	binary="$(installed_binary)"

	install -Dm644 "${REPO}/resources/${APP_ID}.svg" "${icon}"
	write_entry "${binary}" "${entry}"

	echo "Installed:"
	echo "  program       ${binary}"
	echo "  desktop entry ${entry}"
	echo "  icon          ${icon}"
}

# Where cargo install puts a program, in the order cargo itself decides it.
#
# An `install.root` set in a cargo config file is not read here, so the path
# is checked rather than assumed.
installed_binary() {
	local root="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-${HOME}/.cargo}}"
	local binary="${root}/bin/${APP_ID}"
	[ -x "${binary}" ] || die "no program at ${binary};
  run cargo install --path . first, or set CARGO_INSTALL_ROOT to where cargo
  installs for you"
	printf '%s' "${binary}"
}

# The entry names the program by its full path, because a launcher does not
# read the shell's PATH.
write_entry() {
	local binary="$1" entry="$2"

	mkdir -p -- "$(dirname -- "${entry}")"
	sed -e "s|@BINARY@|$(quote_for_exec "${binary}")|" \
		"${REPO}/resources/${APP_ID}.desktop.in" >"${entry}"
	chmod 644 "${entry}"

	if command -v desktop-file-validate >/dev/null; then
		desktop-file-validate "${entry}" || die "wrote an invalid entry to ${entry}"
	fi
}

# A path holding a space or any other reserved character has to be quoted in
# an Exec line, with backslashes and quotes inside it escaped.
quote_for_exec() {
	local path="$1"
	case "${path}" in
	*[[:space:]\"\'\\\$\`\(\)\;\<\>\|\&\#\*\?\~\!]*)
		printf '"%s"' "$(printf '%s' "${path}" | sed -e 's|[\\"`$]|\\&|g')"
		;;
	*) printf '%s' "${path}" ;;
	esac
}

uninstall() {
	local run_cargo="$1"
	shift

	local removed=0
	for path in "$@"; do
		if [ -e "${path}" ]; then
			rm -f -- "${path}"
			echo "Removed ${path}"
			removed=1
		fi
	done
	[ "${removed}" = 1 ] || echo "No entry or icon to remove."

	# The program may well be wanted on the command line after the launcher
	# entry is gone, so a failure here is reported and not fatal.
	if [ "${run_cargo}" != no ]; then
		cargo uninstall "${APP_ID}" || echo "Leaving the program installed."
	fi
}

# A launcher reads its menu from a cache, so it needs telling that the entry
# changed. Both tools are optional, and a missing one only means the entry
# shows up at the next login instead of at once.
refresh_caches() {
	local prefix="$1"
	if command -v update-desktop-database >/dev/null; then
		update-desktop-database -q "${prefix}/share/applications" 2>/dev/null || true
	fi
	if command -v gtk-update-icon-cache >/dev/null; then
		gtk-update-icon-cache -qtf "${prefix}/share/icons/hicolor" 2>/dev/null || true
	fi
}

die() {
	echo "install-desktop-entry: $*" >&2
	exit 1
}

main "$@"
