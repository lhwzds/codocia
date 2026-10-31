---
title: 发布与 npm 包装
source: #95b119baaa79
---

推送 `v*` 标签会运行 Release 工作流。它的预检会拒绝以下标签：不是最新的 `origin/main`、没有成功的 Tests 运行，或版本号与 `Cargo.toml` 或 `npm/package.json` 不一致，所以两者要一起升版本。随后它重跑 `cargo fmt --check`、`cargo test`、npm 测试和两个发布演练，构建五个目标（macOS 和 Linux 的 x64 与 arm64，以及 Windows x64），把压缩包和 `checksums.txt` 附到 GitHub release，再发布 crate 和 npm 包（带 provenance，两者都通过 trusted publishing，不存放 token）。Homebrew tap 每 6 小时自己检查一次新 release 并更新 formula。

npm 包本身不带二进制。它的 `postinstall` 脚本按平台和包自身的版本下载对应压缩包，用 `checksums.txt` 校验 SHA-256，并安装为 `codocia-bin`；`bin/codocia` 只负责启动这个文件。因此 npm 任务要等 GitHub release 任务完成：没有发布资产，安装就会失败。
