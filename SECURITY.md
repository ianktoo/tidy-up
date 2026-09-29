# Security policy

tidy-up moves files, deletes them on request, and can be driven by an agent. Bugs
that cause data loss, or that let it act outside the folder you chose, are treated
as security issues.

## Reporting a vulnerability

Please email **hello@iantoo.space** with:

* what you found and the version (`tidy-up --version`),
* steps to reproduce, ideally on a throwaway folder,
* the impact you expect.

Please don't open a public issue for security problems. You'll get an acknowledgement
within a few days, and we'll coordinate a fix and disclosure with you.

## Threat model

The assumption throughout is that **the folder you point tidy-up at may contain
things you did not write**. You unpack an archive, clone a repository, sync a drive,
or hand a folder to an agent. Anything inside it is data, not instruction.

That single assumption produces the same question in four places, and the answer is
the same each time: *content inside the folder may make tidy-up do less, never more.*

| Surface | What it could try | What stops it |
|---|---|---|
| `.tidy-up.json` in the folder | Declare a policy granting itself permissions | Policy is only read from `--config` or `TIDY_UP_CONFIG`; a policy found in the folder is discarded |
| `.tidy-up/journals/*.jsonl` | Claim a file belongs at an absolute path elsewhere, so `restore` writes outside the folder | Destinations outside the folder are refused unless `--allow-outside-root` is given, and `--yes` alone is not enough |
| A plan file given to `apply` | Name a source or destination outside the plan's root | Every path must be absolute and inside the plan's own root, and the root still passes the system-folder guard |
| An agent over MCP | Ask for a path outside the server root, or supply moves of its own | Everything is confined to `--root`, checked canonically; and an agent supplies no paths at all, only a `plan_id` the server issued |

### What is in scope

* Path handling that escapes the target folder, including `..`, symbolic links,
  junctions, UNC and verbatim prefixes, and 8.3 short names.
* Overwriting or losing files. tidy-up never overwrites: a name clash becomes
  `name (1).ext`.
* Journal corruption or forgery that breaks `restore` or redirects it.
* Anything that lets content inside a folder widen what tidy-up may do.
* The system-folder guard being bypassed other than by the documented flag.
* An agent reaching outside an MCP server's `--root`.

### What is out of scope

* Data loss from running `purge`, which says it cannot be undone and asks first.
* Data loss from deleting `.tidy-up/` yourself, which is documented as removing
  the ability to undo.
* Anyone who already has write access to your files choosing to use tidy-up to
  move them. tidy-up is not a sandbox, and it runs with your permissions.
* Time-of-check to time-of-use races against a local attacker who can modify the
  filesystem while tidy-up runs. The guard defends against mistakes, not against
  someone already on the machine.

## Design rules this follows

Four rules are worth stating, because they are what the mechanisms above are
instances of. They are also the ones to check a change against.

1. **Auto-discovered configuration may lower the bar for convenience; it may
   never raise it for permission.** A setting read from a path someone named is
   authority. The same setting found lying in a folder is a suggestion.
2. **An interface where the caller cannot name a resource beats one that
   validates the names it is given.** Validation is only as complete as its
   author's list of ways a name can escape. The MCP server issues plan handles
   for this reason.
3. **A refusal is stated, never silent.** A forbidden flag is an error rather
   than a quiet no, and an unrecognised config field is rejected rather than
   ignored, so an installation cannot believe it is restricted when it is not.
4. **`--yes` answers a confirmation. It is never an authorisation.** Reaching a
   system folder, or writing outside the folder being restored, needs its own
   flag, and the confirmation still follows.

## What tidy-up does not do

* No network access of any kind. It never checks for updates and has no telemetry.
* No writes outside the folders you name. Run logs and journals live in
  `.tidy-up/` inside the folder that was processed.
* No configuration in your home directory or the registry, unless you pass
  `--config` pointing there.
* No elevation. If it cannot write somewhere, it says so rather than asking for
  more privilege.
