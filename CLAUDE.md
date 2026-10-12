# videre

A fast Rust CLI for managing a local media library: duplicate detection,
semantic search, and face recognition, all around a single SQLite database.

User-facing documentation lives at <https://docs.videre.sh>, generated from
`docs/`. This file is for contributing to videre. When you change behaviour,
update the relevant page under `docs/src/content/docs/` in the same commit.

## Working here

- **Look before you change shared code.** Search the code, tests and docs for
  every caller first. The same thing implemented twice is the worst outcome:
  extend the existing helper instead.
- **Test first when fixing a bug.** Write the test, run it, watch it fail.
- **Verify before claiming done.** Run the commands below and read their
  output.
- **Fixtures look like real libraries:** non-ASCII and Turkish names
  (`Özgür`, `Işıl`, `Gökçe`), not `Alice` and `Bob`.
- **New settings** go in `config.toml` (`videre config set`), never a new
  environment variable.
- **Dependencies are pinned exactly** (`=x.y.z`) to the version in
  `Cargo.lock`, so `cargo install` builds what was tested. To update one,
  change its pin in every crate that uses it, then `cargo update -p <name>`.
- **Every operation leaves the library consistent on its own.** A command or
  gallery action that changes a file or a row also invalidates everything
  derived from it (rows, embeddings, thumbnails) before it returns.
- **Getting a change in.** Stage files by name, never `git add -A`. Open a PR
  against `main`; checks must be green. Keep the PR description short and put
  the reasoning in commit messages. A PR does not bump versions or edit
  `CHANGELOG.md`; releases do that.

## Build and test

```bash
cargo build --release
cargo test --workspace --no-fail-fast
make fmt          # format; use this, not bare `cargo fmt`
make fmt-check    # what CI runs
make lint-check   # clippy with -D warnings, as CI runs it
make lint-linux   # the same lint for Linux, in Docker
```

- The Rust version is pinned in `rust-toolchain.toml`; change it nowhere else.
- `cargo fmt` skips files not reachable through `mod`, so `make fmt` and
  `make fmt-check` hand every file under `crates/` to rustfmt directly.
- CI runs on Linux and macOS. Some code is `cfg(target_os = "macos")` (HEIC
  and video through QuickLook), so a green macOS lint does not prove the Linux
  one: run `make lint-linux` when you touch platform-gated code.
- Tests are model-free by default. `VIDERE_TEST_MODELS=1 make test` runs the
  ones that load real models, which may download weights.

## Layout

```
crates/
  videre/          the binary, one module per subcommand in src/commands/
    tests/         integration tests; tests/common/ is shared
  videre-core/     shared database, cache and search helpers
  videre-ml/       inference (SigLIP, ArcFace/SCRFD)
  videre-api/      the gallery's faces and people operations
docs/              the docs site (Astro Starlight)
e2e/               Playwright tests for the gallery
```

Code needed by another crate goes in `videre-core`; code shared by several
subcommands only goes in a module under `crates/videre/src/`.

## Tests

- Integration tests use `TestLibrary` (`crates/videre/tests/common/`), which
  gives each child process its own library root, `HOME` and `HF_HOME`.
- A test that loads a real model holds the shared `ModelCacheGuard` and uses
  `model_cmd(&guard)`; everything else uses `cmd()`.
- Gallery e2e: `cargo build -p videre`, then `yarn --cwd e2e playwright test`.

## The commit guard

`hooks/pre-commit` (enable with `git config core.hooksPath hooks`) refuses a
commit of more than 30 files, a path outside the repository, or a file that is
not source, config, docs or a fixture. Media belongs only in
`crates/*/tests/fixtures/` or `docs/public/`. Overrides:
`VIDERE_ALLOW_BIG_COMMIT=1`, `VIDERE_ALLOW_ANY_FILE=1`.

## The docs site

```bash
yarn --cwd docs install
yarn --cwd docs dev       # http://localhost:4321
yarn --cwd docs build
```

Yarn 4 with `nodeLinker: node-modules` (PnP does not work with Astro). Node is
pinned by `docs/.node-version`. An unknown Starlight icon name renders an empty
`<svg>` rather than failing the build, so check icon names against the package.
