# Contributing

Issues and pull requests are welcome.

- **Before a PR:** `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` must pass. CI
  runs the same three.
- **Driver changes** go upstream to [SudoVDA](https://github.com/SudoMaker/SudoVDA). This repo only
  pins it as a submodule and builds it.
- **Docs:** a change that makes a file in `docs/` wrong isn't finished until that file is fixed. Paste
  real command output, not a tidied version.
- **Changelog:** add a line under `Unreleased` in `CHANGELOG.md`.
- **Commits:** imperative subject line. Say why in the body when it isn't obvious.

By contributing you agree your work is released under the MIT license.
