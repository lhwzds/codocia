---
title: 参与开发
---

# 参与开发

<sub>[English](../04-contributing.md) · **中文**</sub>

Codocia 是一个 Rust crate，只有两个依赖（`anyhow`、`clap`）；`libc` 只用于在 Unix 上重置 `SIGPIPE`。模块划分见 [代码结构](structure.md)。测试是 `book.rs`、`structure.rs` 和 `lib.rs` 末尾的单元测试。大多数测试在临时目录中建一个一次性 Git 仓库（`fixture`、`init_in_repo`），提交后运行真实命令，因为固定、重命名和提交回退只有在真实历史上才有意义。推送前运行：

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
npm test --prefix npm
```

Tests 工作流在每次推送到 `main` 和每个 pull request 时运行前三项、对这本书的 `codocia check`（完整检出、从源码构建）以及 `cargo publish --dry-run`；发布需要它通过。`SKILL.md` 通过 `include_str!` 编译进二进制，所以修改它会改变 `codocia skill` 的输出，并且需要重新构建。对哈希、frontmatter 格式或 `book.md` 键的任何改动，都会破坏其他仓库中已提交的书；请保持旧格式可读。

<sub>来源: [book.rs:3271-3304](../../src/book.rs#L3271-L3304) · [book.rs:3825-3834](../../src/book.rs#L3825-L3834) · [lib.rs:14-16](../../src/lib.rs#L14-L16) · [main.rs:232-242](../../src/main.rs#L232-L242) · [test.yml](../../.github/workflows/test.yml) · +1</sub>

## 发布与 npm 包装

推送 `v*` 标签会运行 Release 工作流。它的预检会拒绝以下标签：不是最新的 `origin/main`、没有成功的 Tests 运行，或版本号与 `Cargo.toml` 或 `npm/package.json` 不一致，所以两者要一起升版本。随后它重跑 `cargo fmt --check`、`cargo test`、npm 测试和两个发布演练，构建五个目标（macOS 和 Linux 的 x64 与 arm64，以及 Windows x64），把压缩包和 `checksums.txt` 附到 GitHub release，再发布 crate 和 npm 包（带 provenance，两者都通过 trusted publishing，不存放 token）。Homebrew tap 每 6 小时自己检查一次新 release 并更新 formula。

npm 包本身不带二进制。它的 `postinstall` 脚本按平台和包自身的版本下载对应压缩包，用 `checksums.txt` 校验 SHA-256，并安装为 `codocia-bin`；`bin/codocia` 只负责启动这个文件。因此 npm 任务要等 GitHub release 任务完成：没有发布资产，安装就会失败。

<sub>来源: [release.yml:12-94](../../.github/workflows/release.yml#L12-L94) · [release.yml:102-116](../../.github/workflows/release.yml#L102-L116) · [release.yml:148-226](../../.github/workflows/release.yml#L148-L226) · [install.js:138-170](../../npm/scripts/install.js#L138-L170) · [codocia](../../npm/bin/codocia) · +1</sub>
