# Testing strategy

The problem with testing an APFS implementation is getting *trustworthy*
ground truth without a Mac in the loop. This project solves it with two
independent tools from the linux-apfs project (kernel APFS driver authors):

- **mkapfs** formats a genuine, spec-conformant APFS container.
- **apfsck** is a strict independent checker that validates every structure
  (checksums, key ordering, drec name hashes, space-manager accounting,
  extent references, counters...).

The pipeline (`tools/make-fixtures.sh`):

1. `mkapfs` formats a 16 MiB image → a real but empty volume.
2. Our `apfs-fixture` tool injects a known file tree — three files
   (including one spanning three discontiguous extents and one smaller than
   a block), a subdirectory, and a symlink — performing real record
   insertion, block allocation and checksum maintenance.
3. `apfsck` must report the result clean. This is the crucial step: our
   *writer* is checked by an independent implementation, so the *reader*
   tests that follow aren't grading their own homework.
4. The image is gzipped (34 KB) and committed at
   `crates/apfs-core/tests/fixtures/apfs-16m.img.gz`.

`cargo test -p apfs-core` then runs 12 integration tests against that image
covering: metadata counters, root and subdirectory listings, byte-exact
content of single- and multi-extent files, reads at extent-boundary
offsets, EOF handling, symlink targets, case-insensitive lookup, NotFound
errors, child counts, and checksum enforcement across every metadata block
touched by a full walk.

During development the CLI doubles as a harness:

```
apfs info  disk.img          # volumes, counters, flags
apfs tree  disk.img          # recursive listing
apfs cat   disk.img /path    # stream a file
apfs dump-fstree disk.img    # raw record hex (debugging)
```

## Testing against real Mac drives

Fixtures can't cover everything Apple's driver produces (clones, huge
B-trees with multiple levels, decmpfs, snapshots). When you have a real
Mac-formatted drive or a raw image of one:

```
apfs info \\.\PhysicalDrive2       # or the image path
apfs tree \\.\PhysicalDrive2 | more
apfs cat  \\.\PhysicalDrive2 /some/file > out && fc /b out original
```

A read-only pass over a drive you care about is safe by construction — the
tools never open devices for writing.

## Regenerating fixtures (Linux)

```
tools/make-fixtures.sh
```

Requires git, make, gcc. The script clones and builds apfsprogs, formats,
injects, verifies with apfsck, and refreshes the committed .gz only if the
check passes.
