---
title: 结构、翻译与构建
---

# 结构、翻译与构建

<sub>[English](../03-output.md) · **中文**</sub>

## 生成的结构图

智能体写的文件表很快就会过时，所以 Codocia 改为自动生成：每次 `build` 都从代码重新计算 [代码结构](structure.md)，笔记只需链接它，不必罗列模块。`build_map` 取范围内的文件，反复拆分最大的目录，最多分成 30 个区域；较小的同级目录合并为一行 "(other)"。

每个区域会显示：区域自带的一行描述（取自 `Cargo.toml` 或 `package.json` 元数据、README、Python 包的 docstring 或 Rust 的 `//!` 注释），三个枢纽文件（先按被导入次数，再按大小排序；排除测试，因为测试几乎导入一切），以及哪些页面的笔记引用了它。一张 Mermaid 图展示区域之间最强的导入依赖。

`import_edges` 按语言把导入解析到仓库内的文件（Python、Rust `use` 路径、JS/TS、Go、C 系 include）；外部依赖直接丢弃。没有解析器的语言就没有依赖边。

<sub>来源: [structure.rs:1-5](../../src/structure.rs#L1-L5) · [structure.rs:34-130](../../src/structure.rs#L34-L130) · [structure.rs:329-350](../../src/structure.rs#L329-L350) · [book.rs:2527-2533](../../src/book.rs#L2527-L2533) · [commit 38c44b1](https://github.com/lhwzds/codocia/commit/38c44b1) · +2</sub>

## 不会悄悄漂移的翻译

Codocia 从不翻译；它只记录每份翻译是根据哪个源文本做的。书的翻译包括 `<lang>/book.md`（标题、页面标题、引言）和每条笔记对应的 `<lang>/<page>/<id>.md`，frontmatter 中带 `source: #hash`；哈希覆盖源笔记的标题和正文，不含证据，所以重新固定代码不会使译文失效。仓库文件的翻译对写在 `book.md` 中，形如 `README_CN.md: README.md #hash`。

翻译缺失、从未同步、源哈希已变，或所译笔记的源已被删除时，`check` 报错。之后 `parity` 比较机械结构：标题数量、代码块（忽略注释）、ASCII 行内代码或外部 URL 不一致时发出警告，因为这些在每种语言中都必须相同。`accept zh`（或 `zh/book`、`zh/page/id`、`README_CN.md`）会运行 `sync`，写入当前的源哈希，并拒绝符号链接或逃出目录的文件；它不评判译文质量。

<sub>来源: [translate.rs:1-13](../../src/translate.rs#L1-L13) · [translate.rs:132-147](../../src/translate.rs#L132-L147) · [translate.rs:190-261](../../src/translate.rs#L190-L261) · [translate.rs:295-328](../../src/translate.rs#L295-L328) · [translate.rs:470-528](../../src/translate.rs#L470-L528) · +1</sub>

## build 写出什么

`build` 先运行 `follow` 和完整检查，有任何错误就停止，除非给出 `--force`。输出目录依次取 `--out`、`book.md` 中的 `out:` 路径（相对于仓库，不能含 `..`），否则 `.codocia/` 中的书写到 `docs/`，外部书写到 `<book>/out`。凡不是用 `--out` 指定的输出目录，都必须留在仓库内（外部书则在书目录内），且路径上不能有符号链接；只有显式的 `--out` 可以指向别处。它为每页写一个 `NN-slug.md`，外加 `structure.md`，以及把所有页面合在一页的 `all.md`；在仓库内部时索引名为 `README.md`，让该目录在 GitHub 上直接显示。`langs` 中的每种语言在 `<lang>/` 子目录下生成同样一套文件，每个标题下有语言切换。

在仓库内部，源码链接是相对路径（`../src/book.rs#L10-L20`），索引只写远程仓库名，从不写本地路径。`.codocia-build` 列出所有生成的文件。`prepare_output` 先检查每个目标路径和标记文件（不允许符号链接，不能逃出目录，标记文件不是清单时构建直接停止），然后只删除清单中的文件，并拒绝覆盖任何其他文件，因此手写页面可以和书共用 `docs/`。

<sub>来源: [book.rs:1169-1238](../../src/book.rs#L1169-L1238) · [book.rs:1432-1442](../../src/book.rs#L1432-L1442) · [book.rs:2870-2966](../../src/book.rs#L2870-L2966) · [book.rs:3195-3215](../../src/book.rs#L3195-L3215) · [translate.rs:545-595](../../src/translate.rs#L545-L595) · +3</sub>

## 移动固定提交

`book.md` 记录笔记最后一次核验所对应的提交；它是 `accept` 的“变动前”版本，也是重命名检测的基准。每次 `build`（包括带 `--force` 时）都会用 `move_book_commit` 移动它：从 `HEAD` 开始沿第一父提交回退，只要某个提交只改动了被忽略的文件、书本身、仓库翻译文件或 Codocia 生成的输出，就继续回退。

没有这一步，每次提交刚构建的 `docs/` 或修正 `README_CN.md` 都会让固定提交落后一个，`status` 也会在只有文档变化时报告代码已变。`code_moved` 以同样方式跳过生成的输出和被忽略的文件，所以只添加构建文档的提交不会被 `status` 标记。

<sub>来源: [book.rs:1091-1109](../../src/book.rs#L1091-L1109) · [book.rs:2755-2790](../../src/book.rs#L2755-L2790) · [commit ed8fc0e](https://github.com/lhwzds/codocia/commit/ed8fc0e) · [commit b8905c3](https://github.com/lhwzds/codocia/commit/b8905c3)</sub>
