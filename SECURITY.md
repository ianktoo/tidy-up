# Security policy

tidy-up moves and (via `purge`) deletes files, so bugs that could cause data loss
or act outside the chosen folder are treated as security issues.

## Reporting a vulnerability

Please email **hello@iantoo.space** with:

* what you found and the version (`tidy-up --version`),
* steps to reproduce, ideally on a throwaway folder,
* the impact you expect.

Please don't open a public issue for security problems. You'll get an acknowledgement
within a few days, and we'll coordinate a fix and disclosure with you.

## Scope

In scope: path handling that could escape the target folder, overwriting or losing
files, journal corruption that breaks `restore`, unsafe handling of symlinks.

Out of scope: data loss caused by deleting `.tidy-up/` or by running `purge`
(documented behavior).
