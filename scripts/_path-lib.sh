# shellcheck shell=sh
# Shared helpers for add-to-path.sh and remove-from-path.sh.
# Plain POSIX sh: works in dash, bash and zsh (and macOS's old bash 3.2).
# This file is sourced, not run. It is ASCII only on purpose.

BEGIN_MARK='# >>> tidy-up >>>'
END_MARK='# <<< tidy-up <<<'
dry=${dry:-0}

say() { printf '%s\n' "$*"; }
die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

# Writes a directory the way a shell file should contain it: as $HOME/... when it is under
# the home directory, so the line keeps working if the home directory is ever moved.
display_dir() {
    case "$1" in
        "$HOME"/*)
            # shellcheck disable=SC2016
            printf '$HOME/%s' "${1#"$HOME"/}"
            ;;
        *) printf '%s' "$1" ;;
    esac
}

# The shell file(s) to edit for a given shell name.
rc_files_for() {
    case "$1" in
        zsh) printf '%s\n' "$HOME/.zshrc" ;;
        bash)
            printf '%s\n' "$HOME/.bashrc"
            if [ "$(uname -s)" = Darwin ]; then
                printf '%s\n' "$HOME/.bash_profile"
            fi
            ;;
        fish) printf '%s\n' "$HOME/.config/fish/conf.d/tidy-up.fish" ;;
        *) printf '%s\n' "$HOME/.profile" ;;
    esac
}

# Every file the add script might have edited (used by the remove script).
all_rc_files() {
    printf '%s\n' \
        "$HOME/.zshrc" \
        "$HOME/.bashrc" \
        "$HOME/.bash_profile" \
        "$HOME/.profile" \
        "$HOME/.config/fish/conf.d/tidy-up.fish"
}

# The marked block for a shell. The markers are what make removal exact.
block_for() {
    shown=$(display_dir "$2")
    printf '%s\n' "$BEGIN_MARK"
    case "$1" in
        fish)
            # shellcheck disable=SC2016
            printf 'if not contains -- "%s" $PATH\n    set -gx PATH "%s" $PATH\nend\n' "$shown" "$shown"
            ;;
        *)
            # shellcheck disable=SC2016
            printf 'export PATH="%s:$PATH"\n' "$shown"
            ;;
    esac
    printf '%s\n' "$END_MARK"
}

has_block() {
    [ -f "$1" ] && grep -qF "$BEGIN_MARK" "$1"
}

# add_block FILE SHELL DIR
add_block() {
    file=$1
    if has_block "$file"; then
        say "  already set up in $file"
        return 0
    fi
    if [ "$dry" = 1 ]; then
        say "  would add to $file"
        return 0
    fi
    mkdir -p "$(dirname -- "$file")"
    # keep the last existing line intact if the file does not end with a newline
    if [ -s "$file" ] && [ -n "$(tail -c 1 "$file")" ]; then
        printf '\n' >>"$file"
    fi
    block_for "$2" "$3" >>"$file"
    say "  added to $file"
}

# remove_block FILE
remove_block() {
    file=$1
    if ! has_block "$file"; then
        return 0
    fi
    if [ "$dry" = 1 ]; then
        say "  would remove from $file"
        return 0
    fi
    tmp=$(mktemp "${TMPDIR:-/tmp}/tidy-up.XXXXXX")
    awk -v b="$BEGIN_MARK" -v e="$END_MARK" \
        '$0 == b { skip = 1; next } skip && $0 == e { skip = 0; next } !skip { print }' \
        "$file" >"$tmp"
    cat "$tmp" >"$file"
    rm -f "$tmp"
    case "$file" in
        */conf.d/tidy-up.fish)
            # a file we created and that is now empty is ours to delete
            if [ ! -s "$file" ]; then
                rm -f "$file"
            fi
            ;;
    esac
    say "  removed from $file"
}
