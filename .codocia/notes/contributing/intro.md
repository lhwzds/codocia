---
title: 
order: 0
evidence:
  - src/book.rs:3038-3071 #a16be81e10b0
  - src/book.rs:3479-3488 #e3d1cb3d2c62
  - src/lib.rs:14-16 #46689c54866c
  - src/main.rs:232-242 #fb72c76bb572
  - .github/workflows/test.yml #c12345db6290
  - Cargo.toml #12a81cffe5b0
---

Codocia is one Rust crate with two dependencies (`anyhow`, `clap`); `libc`
only resets `SIGPIPE` on Unix. The modules are in [[structure]]. Tests are
unit tests at the bottom of `book.rs`, `structure.rs` and `lib.rs`. Most build a throwaway Git
repository in the temp directory (`fixture`, `init_in_repo`), commit to it,
and run the real commands, because pinning, renames and the commit walk
only mean something against real history. Before pushing:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm test --prefix npm
```

The Tests workflow runs the first three and `cargo publish --dry-run` on
every push to `main` and every pull request. `SKILL.md` is compiled into
the binary with `include_str!`, so editing it changes `codocia skill`
output and needs a rebuild. Any change to the hash, the frontmatter format
or the `book.md` keys breaks books already committed in other
repositories; keep old forms readable.
