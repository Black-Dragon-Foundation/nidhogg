# AGENTS.md — Nidhogg — agent rules

This is Nidhogg, the Black Dragon Foundation's downstream build of GNU Emacs 31. Its changes live in a patch series
on top of an upstream release tag (see `README.md`). This file replaces upstream's `AGENTS.md`.

## What you may do

- Work on Nidhogg's patch series: code, tests and docs, under the conventions in `README.md`.
- Every commit subject starts with `nidhogg: `, and the series sits on the upstream release tag.
- Before finishing, `bash nidhogg/test/tree-test.sh` must pass.

## Never send anything to GNU

Nidhogg's patches are developed with AI assistance, and GNU Emacs's own policy is that it does not accept
LLM-generated patches, bug reports or planning. Nidhogg respects that by not contributing upstream at all:

- No patches, no bug reports (no `M-x report-emacs-bug`, no debbugs), no mailing-list posts, no contact with
  emacs-devel or any other GNU list or tracker.
- Do not draft text meant for GNU, even for a human to send. If a bug is in upstream code, fix it in Nidhogg's series.
