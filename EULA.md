# tidy-up End User License Agreement

**Version 1.0 — DRAFT: have this reviewed by a lawyer before you rely on it.**

This End User License Agreement ("Agreement") applies to binary distributions
of **tidy-up** ("the Software") that you obtain from the publisher ("Ian T.",
"we"). By installing or using such a binary you accept this Agreement.

## Relationship to the MIT License

The **source code** of tidy-up is published on GitHub under the MIT License
(see [`LICENSE`](LICENSE)). The MIT License alone governs your use of the
source code, including any binary you build yourself from it. This Agreement
governs only official pre-built binaries and the tidy-up name.

If anything in this Agreement conflicts with the MIT License as it applies to
the source code, the MIT License prevails for the source code.

## 1. Grant

We grant you a non-exclusive, worldwide, revocable licence to install and use
official binaries of the Software on any number of devices you own or control,
for personal, educational or commercial purposes.

## 2. Restrictions

You may not, except as permitted by the MIT License for the source code or by
law:

1. sell, rent or sublicense official binaries as a standalone product;
2. remove or alter copyright or licence notices;
3. present modified builds as official tidy-up releases, or use the tidy-up
   name or logo in a way that suggests our endorsement.

## 3. Your files — read this

tidy-up **moves files** on your computer. It is designed to be safe: it never
overwrites files, previews changes, asks before acting and records every move
in a journal (`.tidy-up/`) so a run can be undone with `tidy-up restore`.
However:

* Undo depends on the journal. If you delete `.tidy-up/`, edit its contents, or
  delete/modify files that tidy-up moved, undo may be partial or impossible.
* `tidy-up purge` **permanently deletes** files. That cannot be undone.
* You are responsible for keeping backups of important data and for reviewing
  the plan before confirming it.

## 4. Privacy

The Software runs locally. It does not collect, transmit or share your files,
file names or usage data, and makes no network connections.

## 5. No warranty

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE, NON-INFRINGEMENT, OR THAT IT WILL BE ERROR-FREE OR THAT ANY DATA WILL
BE PRESERVED OR RECOVERABLE.

## 6. Limitation of liability

TO THE MAXIMUM EXTENT PERMITTED BY LAW, WE ARE NOT LIABLE FOR ANY INDIRECT,
INCIDENTAL, SPECIAL OR CONSEQUENTIAL DAMAGES, INCLUDING LOSS OF DATA, ARISING
FROM USE OF THE SOFTWARE. OUR TOTAL LIABILITY FOR ANY CLAIM IS LIMITED TO THE
AMOUNT YOU PAID FOR THE SOFTWARE (ZERO IF YOU OBTAINED IT FREE OF CHARGE).
Nothing in this Agreement excludes liability that cannot be excluded by law.

## 7. Termination

This Agreement ends automatically if you breach it. On termination you must stop
using official binaries. Sections 3, 5 and 6 survive termination.

## 8. General

This Agreement is the entire agreement about official binaries. If a provision
is unenforceable, the rest remains in effect. Governing law and venue: *to be
completed by the publisher.*
