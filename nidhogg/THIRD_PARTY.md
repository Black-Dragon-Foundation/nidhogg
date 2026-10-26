# Third-party sources

Files derived from another project's code carry an `Adapted-from: <source>` line in their header. `<source>` is the
`##` heading of an entry below; `nidhogg/test/tree-test.sh` checks both directions (every `Adapted-from` names an
entry, every entry is named by at least one file).

Format of an entry, one `##` section per source:

    ## <name or URL used in the Adapted-from line>

    - URL: where the code lives
    - Commit: the revision the code was taken from
    - Licence: the SPDX identifier of the licence it was taken under
    - Files: the files in this tree derived from it

Entries:

(none yet)
