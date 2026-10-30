---
title: build 写出什么
source: #eacec16ab5d1
---

`build` 先运行 `follow` 和完整检查，有任何错误就停止，除非给出 `--force`。输出目录依次取 `--out`、`book.md` 中的 `out:` 路径（相对于仓库，不能含 `..`），否则 `.codocia/` 中的书写到 `docs/`，外部书写到 `<book>/out`。它为每页写一个 `NN-slug.md`，外加 `structure.md`，以及把所有页面合在一页的 `all.md`；在仓库内部时索引名为 `README.md`，让该目录在 GitHub 上直接显示。`langs` 中的每种语言在 `<lang>/` 子目录下生成同样一套文件，每个标题下有语言切换。

在仓库内部，源码链接是相对路径（`../src/book.rs#L10-L20`），在任何检出或 fork 中都能用；索引只写远程仓库名，从不写本地路径。`.codocia-build` 列出所有生成的文件。下次构建只删除这些文件，并拒绝覆盖任何同名的非生成文件，因此手写页面可以和书共用 `docs/`。
