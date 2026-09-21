#!/bin/sh
# Undoes add-to-path.sh: removes the marked block from your shell startup files and deletes
# the installed tidy-up executable. Works on macOS and Linux, no root needed.
#
#   sh scripts/remove-from-path.sh              # undo everything add-to-path.sh did
#   sh scripts/remove-from-path.sh --dry-run    # show what would happen, change nothing
#   sh scripts/remove-from-path.sh --keep-binary
#
# It only removes what add-to-path.sh added. Your own lines in your shell files, and any
# other program installed in the same folder, are left exactly as they were. Your files are
# never touched, and the undo journals (.tidy-up folders) stay so you can still restore runs.
set -eu

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
# shellcheck source=_path-lib.sh
. "$here/_path-lib.sh"

usage() {
    cat <<'EOF'
Usage: remove-from-path.sh [options]

Options:
  --dir DIR       the folder tidy-up was installed to (default: ~/.local/bin)
  --keep-binary   remove the PATH setup but leave the executable in place
  --dry-run       print what would be done and change nothing
  -h, --help      show this help
EOF
}

dir="$HOME/.local/bin"
keep_binary=0
dry=0

while [ $# -gt 0 ]; do
    case "$1" in
        --dir)
            [ $# -ge 2 ] || die "--dir needs a value"
            dir=$2
            shift 2
            ;;
        --keep-binary)
            keep_binary=1
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

say "Removing the PATH setup:"
all_rc_files | while IFS= read -r rc; do
    remove_block "$rc"
done

target="$dir/tidy-up"
if [ "$keep_binary" = 1 ]; then
    say "Kept $target (--keep-binary)."
elif [ -f "$target" ]; then
    if [ "$dry" = 1 ]; then
        say "  would delete $target"
    else
        rm -f "$target"
        say "  deleted $target"
    fi
else
    say "  no executable at $target"
fi

say ""
if [ "$dry" = 1 ]; then
    say "Dry run: nothing was changed."
else
    say "Done. Open a NEW terminal so the change takes effect."
    say "Leftover .tidy-up folders (undo journals) inside folders you processed were not touched."
fi
