# Technical paper

*Guarded Bulk File Reorganization: System-Directory Detection, Survivable Execution, and
Reversible Re-grouping* — Ian Too
([ORCID 0009-0000-4888-1941](https://orcid.org/0009-0000-4888-1941)).

The paper documents the safety, survivability, re-grouping and observability work in
this repository: what was built, why each choice was made, what was deliberately left
out, and how each property is tested.

## Building

```sh
make          # pdflatex, three passes
make lua      # lualatex  (recommended)
make xe       # xelatex
make check    # fail if the last build logged an error or a missing glyph
make clean
```

Any of the three engines works. The preamble detects which one is running:

- **pdfLaTeX** uses `inputenc`/`fontenc`. Accented Latin text is typeset normally; the
  one sample outside Latin is transliterated and marked, because a Latin font cannot
  render it.
- **LuaLaTeX / XeLaTeX** use `fontspec` and look for a CJK face in turn (Noto Serif
  CJK JP, Noto Sans CJK JP, Hiragino Mincho ProN, Yu Mincho, MS Mincho, SimSun). The
  first one installed is used for that sample; if none is, the build falls back to the
  same transliteration rather than failing.

Both paths are verified: on the author's machine, pdfLaTeX and LuaLaTeX each produce
19 pages with zero errors, zero warnings and zero missing glyphs.

`orcidlink` is used for the ORCID mark when installed, and a plain hyperlink otherwise,
so no package outside a standard TeX distribution is required.

## Figures quoted in the paper

Every number comes from this repository at the commit the paper describes:

| Figure | Source |
|---|---|
| 363 tests, zero failures | `cargo test` |
| Zero lint warnings | `cargo clippy --all-targets` |
| 10 direct / 54 transitive dependencies | `cargo tree` |
| 6,195 lines added | `wc -l` over the modules listed in the availability section |
| 2.24 MiB release binary | `cargo build --release` (LTO, stripped) |
