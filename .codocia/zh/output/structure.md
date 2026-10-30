---
title: 生成的结构图
source: #1be91bb3f46c
---

智能体写的文件表很快就会过时，所以 Codocia 改为自动生成：每次 `build` 都从代码重新计算 [[structure]]，笔记只需链接它，不必罗列模块。`build_map` 取范围内的文件，反复拆分最大的目录，最多分成 30 个区域；较小的同级目录合并为一行 "(other)"。

每个区域会显示：区域自带的一行描述（取自 `Cargo.toml` 或 `package.json` 元数据、README、Python 包的 docstring 或 Rust 的 `//!` 注释），三个枢纽文件（先按被导入次数，再按大小排序；排除测试，因为测试几乎导入一切），以及哪些页面的笔记引用了它。一张 Mermaid 图展示区域之间最强的导入依赖。

`import_edges` 按语言把导入解析到仓库内的文件（Python、Rust `use` 路径、JS/TS、Go、C 系 include）；外部依赖直接丢弃。没有解析器的语言就没有依赖边。
