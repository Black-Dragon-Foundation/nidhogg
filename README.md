# Nidhogg

## What Nidhogg is

Nidhogg is drgnx's own Emacs: GNU Emacs 31 plus a small, linear series of patches. Packages keep working, because
Nidhogg is still Emacs. It is licensed GPL-3.0-or-later, like Emacs. The patches are not meant for upstreaming.

## Identity

- Command: `nidhogg` (and `nidhoggclient`). Feature: `nidhogg`. Variable: `nidhogg-version`.
- `emacs-version` stays GNU Emacs's own (`"31.1"`): packages call `version<` on it, and `version-to-list` rejects
  `"31.1-nidhogg.1"`. Nidhogg's version lives in `nidhogg-version` and is appended to `M-x emacs-version` output.

## Version scheme

`<GNU Emacs version>-nidhogg.<N>`. N counts Nidhogg releases on one upstream release and restarts at 1 on the next
(31.2 gives `31.2-nidhogg.1`). The git tag is `v31.1-nidhogg.1`. The single source is the file `nidhogg/version`.

## Branches and the patch series

- `emacs-31` is an exact mirror of upstream's `emacs-31`; it is never committed to.
- `nidhogg-31` is the series, rebased onto the upstream release tag (`emacs-31.1` for the first release).
- Every commit on `emacs-31.1..nidhogg-31` has a subject starting `nidhogg: `.
- Files only Nidhogg owns live under `nidhogg/`, `.github/` and `README.md`, so rebases do not conflict.
- C patches sit behind `--with-nidhogg-<feature>` configure flags. The identity patch is unconditional.
- `bash nidhogg/test/tree-test.sh [BASE]` checks these rules.

## Rebasing onto a new upstream release

```bash
git fetch upstream emacs-31 tag emacs-31.2 --no-tags   # and fast-forward the emacs-31 mirror
git tag v31.1-nidhogg.N nidhogg-31                       # the old series stays reachable (already tagged at release)
git rebase --onto emacs-31.2 emacs-31.1 nidhogg-31
echo 31.2-nidhogg.1 > nidhogg/version                    # amend into the tree commit
bash nidhogg/test/tree-test.sh emacs-31.2
```
