---
title: 
source: #c7d9912f29ee
---

Codocia 是一个 Rust crate，只有两个依赖（`anyhow`、`clap`）；`libc` 只用于在 Unix 上重置 `SIGPIPE`。模块划分见 [[structure]]。测试是 `book.rs`、`structure.rs` 和 `lib.rs` 末尾的单元测试。大多数测试在临时目录中建一个一次性 Git 仓库（`fixture`、`init_in_repo`），提交后运行真实命令，因为固定、重命名和提交回退只有在真实历史上才有意义。推送前运行：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm test --prefix npm
```

Tests 工作流在每次推送到 `main` 和每个 pull request 时运行前三项和 `cargo publish --dry-run`。`SKILL.md` 通过 `include_str!` 编译进二进制，所以修改它会改变 `codocia skill` 的输出，并且需要重新构建。对哈希、frontmatter 格式或 `book.md` 键的任何改动，都会破坏其他仓库中已提交的书；请保持旧格式可读。
