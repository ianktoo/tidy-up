#!/bin/sh
# Installs tidy-up for the current user and makes the `tidy-up` command available in new
# terminals. Undo it with remove-from-path.sh. Works on macOS and Linux, no root needed.
#
#   sh scripts/add-to-path.sh                 # install to ~/.local/bin and add it to PATH
#   sh scripts/add-to-path.sh --dry-run       # show what would happen, change nothing
#   sh scripts/add-to-path.sh --help
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=_path-lib.sh
. "$here/_path-lib.sh"

usage() {
    cat <<'EOF'
Usage: add-to-path.sh [options]

Copies the tidy-up executable into a folder and, if that folder is not already on your PATH,
adds it to your shell startup file inside a marked block that remove-from-path.sh can delete.

Options:
  --binary FILE   the tidy-up executable to install
                  (default: ../tidy-up or ./tidy-up next to this script)
  --dir DIR       where to install it (default: ~/.local/bin)
  --shell NAME    zsh, bash, fish or sh (default: taken from $SHELL)
  --no-rc         install the executable but do not touch any shell file
  --dry-run       print what would be done and change nothing
  -h, --help      show this help
EOF
}

dir="$HOME/.local/bin"
binary=""
shell_name=""
no_rc=0
dry=0

while [ $# -gt 0 ]; do
    case "$1" in
        --binary)
            [ $# -ge 2 ] || die "--binary needs a value"
            binary=$2
            shift 2
            ;;
        --dir)
            [ $# -ge 2 ] || die "--dir needs a value"
            dir=$2
            shift 2
            ;;
        --shell)
            [ $# -ge 2 ] || die "--shell needs a value"
            shell_name=$2
            shift 2
            ;;
        --no-rc)
            no_rc=1
            shift
            ;;
        --dry-run)
            dry=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *) die "unknown option: $1 (try --help)" ;;
    esac
done

if [ -z "$binary" ]; then
    for candidate in "$here/../tidy-up" "$here/tidy-up"; do
        if [ -f "$candidate" ]; then
            binary=$candidate
            break
        fi
    done
fi
if [ -z "$binary" ] || [ ! -f "$binary" ]; then
    die "cannot find the tidy-up executable; run this from the extracted release folder or pass --binary FILE"
fi

if [ -z "$shell_name" ]; then
    shell_name=$(basename -- "${SHELL:-sh}")
fi

target="$dir/tidy-up"
abs_binary=$(CDPATH='' cd -- "$(dirname -- "$binary")" && pwd)/$(basename -- "$binary")

say "Installing tidy-up:"
if [ "$abs_binary" = "$target" ]; then
    say "  $target is already in place"
elif [ "$dry" = 1 ]; then
    say "  would copy $binary to $target"
else
    mkdir -p "$dir"
    install -m 755 "$binary" "$target"
    say "  copied to $target"
    if [ "$(uname -s)" = Darwin ]; then
        # lets a downloaded, unsigned binary run without the Gatekeeper prompt
        xattr -d com.apple.quarantine "$target" 2>/dev/null || true
    fi
fi

if [ "$no_rc" = 1 ]; then
    say "Skipped changing any shell file (--no-rc). Add $dir to your PATH yourself."
else
    case ":$PATH:" in
        *":$dir:"*)
            say "$dir is already on your PATH, so no shell file needs changing."
            ;;
        *)
            say "Adding $dir to your PATH for $shell_name:"
            rc_files_for "$shell_name" | while IFS= read -r rc; do
                add_block "$rc" "$shell_name" "$dir"
            done
            ;;
    esac
fi

say ""
if [ "$dry" = 1 ]; then
    say "Dry run: nothing was changed."
else
    say "Done. Open a NEW terminal, then check it:  tidy-up --version"
    say "To undo all of this:  sh $here/remove-from-path.sh"
fi
