# craft-pdf, vendored

Base: `8e03cd016d42032b867fef6bad86bc69bf2c1920` (storytold/craft-libs `crates/pdf`, MIT OR Apache-2.0).

Upstream PdfCraft moved its object layer (`pdfcraft-cos`, `-crypt`, `-filters` and `-content`)
into craft-pdf, and those 4 crates became one-line facades over it. This fork had fixed that code
first, so craft-pdf is vendored here with the fixes applied, and the root `Cargo.toml` swaps it in
with `[patch."https://github.com/storytold/craft-libs"]`. `cargo xtask fork-check` fails when the
craft-pdf revision `Cargo.toml` names is not the base above, since Cargo would otherwise keep
building this copy after a revision bump.

## What differs from the base

- `Cargo.toml`: the workspace-inherited fields, lints and dependencies are written out from the
  craft-libs workspace at the base, `publish = false` is added, and the sibling
  `craft-stream-codecs` and `craft-svg` path dependencies become git dependencies at the base. The
  base's manifest is `Cargo.toml.orig`.
- `LICENSE-APACHE`, `LICENSE-MIT` and `NOTICE` are craft-libs' own, from its root at the base.
- `patches/0001-pdfcraft-object-layer.patch` holds every source change, in these files:

| Fix | Files | Tests |
| --- | --- | --- |
| A Document Open password is asked for rather than the file opened owner-first (#603, `45444a9b`, `7033a2e9`) | `src/crypt.rs`, `src/cos/document/open.rs` | `crypt::tests::a_non_empty_user_password_is_reported_as_mandatory`; `crates/cos/tests/it/encryption.rs` |
| A crypt filter named by a reference that can't be read is never `/Identity`, and a stream is never saved in the clear through a filter `/CF` doesn't hold (#63, #66, `4af80e92`, `68391f05`) | `src/cos/security.rs`, `src/cos/document/open.rs`, `src/cos/document/tests.rs` | `crates/cos/tests/it/encryption.rs` |
| `Document::try_resolve` keeps the error of a reference that can't be loaded, for the redaction audit (#76, `0e676540`) | `src/cos/document/objects.rs` | `crates/audit` tests |
| The weak-hash lines carry `NOSONAR` (#58) | `src/crypt.rs` | `cargo xtask fork-check` |
| Failed operands are read again a bounded number of times (#128, `f74779bb`) | `src/cos/content.rs` | `cos::content::tests::unclosed_dictionaries_and_arrays_are_read_a_bounded_number_of_times` |
| A written inline image ends where it was read, as the renderer reads it (#134, `9ab4f813`) | `src/cos/content.rs` | the `cos::content::tests` inline-image cases |
| `Document::scan_objects_where` judges an encrypted file's objects as stored and decrypts only those it keeps, so the signature list's search for document timestamps no longer decrypts the whole file (#160) | `src/cos/document/objects.rs`, `src/cos/document/tests.rs` | `cos::document::tests::a_refused_object_is_not_loaded_through_its_own_length`; `crates/cos/tests/it/encryption.rs`; `crates/sign` `discovery_does_not_decrypt_what_it_refuses` |

Upstream security commits the fork took early (#67) are in the base already. RC4 stays craft-pdf's
own: the fork's RustCrypto `rc4` (#37) was listed under 'Fixed differently upstream' in the root
`PATCHES.md`, and its property test against the replaced code still runs
(`crates/crypt/tests/it/proptests.rs`).

## Moving to another revision

1. Copy `crates/pdf` at the new revision over this folder, keeping `PATCHES.md` and `patches/`.
2. Write out its manifest as above, and save the new base manifest as `Cargo.toml.orig`.
3. From this folder, `git apply -3 patches/*.patch`; resolve any conflict, then regenerate the
   patch against the new base.
4. Set the `craft-pdf` revision in the root `Cargo.toml` and the base above to the new commit.
5. Run its unit tests, `cargo xtask fork-check` and `cargo xtask ci`.

## Its unit tests

Like the rest of `vendor/`, this crate is outside the workspace: as a member, every optional
dependency of its other features would join the root `Cargo.lock`. It has its own `Cargo.lock`
instead, and `cargo xtask ci`'s `craft-pdf` step runs its unit tests, the fixes' among them, on that
lockfile and checks it with cargo-deny against the root `deny.toml`. Cargo run from this folder
takes the workspace's hayro-syntax and hayro-jbig2 from `.cargo/config.toml`:

```sh
cd vendor/craft-pdf
CARGO_TARGET_DIR=../../target cargo test --locked --features content --lib
cargo deny --locked --features content check
```

After a move to another revision, refresh the lockfile from the root's, so shared crates keep the
workspace's versions: copy `../../Cargo.lock` here and run `cargo metadata --format-version 1 >
/dev/null`, which drops what craft-pdf does not use and adds what it needs.
