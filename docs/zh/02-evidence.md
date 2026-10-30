---
title: 证据与审阅
---

# 证据与审阅

<sub>[English](../02-evidence.md) · **中文**</sub>

笔记的证据是 frontmatter 中的一组条目，形式为 `path`、`path:N`、`path:start-end`、`commit:<sha>`、`pr:<n>` 或 `issue:<n>`。路径必须相对于仓库根目录；提交至少要有七个十六进制字符。文件条目第一次保存时，`pin_for` 会追加 ` #hash`：对所引行（去掉行尾空白）计算 64 位 FNV-1a 哈希（`stable_hash_hex`），取前 12 个十六进制位；没有给出行范围时对整个文件计算。本页的一切都来自把这个哈希与当前文件比较。

这个哈希有意做得简单而稳定：不依赖任何库，在所有平台上结果一致，也不受行尾格式变化影响。改动 `hash_lines` 会让所有现有书里的每个固定值都变成“代码已变”错误。

<sub>来源: [book.rs:1727-1780](../../src/book.rs#L1727-L1780) · [book.rs:1803-1819](../../src/book.rs#L1803-L1819) · [book.rs:1915-1922](../../src/book.rs#L1915-L1922) · [lib.rs:36-43](../../src/lib.rs#L36-L43)</sub>

## 跟随移动的代码

`evidence_state` 判断一个已固定的范围现在处于什么状态。若哈希在所引行处仍然匹配，就是 `Ok`。否则它在文件上滑动一个等长窗口；在别处匹配到就是 `Moved`，并取离原位置最近的匹配，所以上方插入代码导致下移的函数能被重新找到。完全没有匹配则是 `Stale`（代码已变）或 `BadRange`（文件变短了）。

`follow` 在 `check`、`accept` 和 `build` 开始时运行，并就地改写笔记文件：移动过的范围更新为新行号，未固定的条目补上固定值。文件不存在时，先在 `git_renames` 中查找；它从书的固定提交起运行 `git diff -M`，所以重命名的文件不会丢失证据。提交 `fa187ba` 把原来的 `pin` 命令并入这一步，只有真正改动的代码才需要人来处理。

<sub>来源: [book.rs:1866-1912](../../src/book.rs#L1866-L1912) · [book.rs:827-867](../../src/book.rs#L827-L867) · [book.rs:2578-2609](../../src/book.rs#L2578-L2609) · [commit 8c83c23](https://github.com/lhwzds/codocia/commit/8c83c23) · [commit fa187ba](https://github.com/lhwzds/codocia/commit/fa187ba)</sub>

## 用 accept 审阅变动的代码

`Stale` 条目在 `check` 中一直是错误，直到有人重读代码、必要时修改笔记，并运行 `codocia accept page/id`。对每个过期范围，`accept` 打印变动前后的代码行。“变动前”取自固定提交（文件改过名时用旧名）：由于 `follow` 可能已经改了行号，`pinned_lines` 在那个版本上滑动窗口，找出哈希等于固定值的那些行。

接着是范围检查：如果旧版本中含字母或数字的行（去掉首尾空白）在新范围中出现的比例不到 30%（每个新行最多匹配一个旧行），说明该范围现在覆盖的是另一段代码，例如函数被替换或移出了范围，`accept` 会拒绝。固定版本中没有任何窗口与固定值匹配时，它同样拒绝，因此总是和经过验证的文本比较。正确做法是在笔记里改正范围；`--force` 只用于已手工核对过的范围，会跳过检查。只要有一个范围被拒绝，整条笔记都保留原来的固定值。

<sub>来源: [book.rs:923-1000](../../src/book.rs#L923-L1000) · [book.rs:1043-1062](../../src/book.rs#L1043-L1062) · [commit 2658f2d](https://github.com/lhwzds/codocia/commit/2658f2d)</sub>

## check 的其他规则

错误会让 `check` 以非零状态退出，CI 正依赖这一点：证据过期、缺失或超出范围，笔记没有证据或正文，页面未列在 `book.md` 中，页面链接指向不存在的页面，以及翻译漂移（[结构、翻译与构建](03-output.md)）。其余都是警告。

`symbol_warnings` 是防止编造名称的廉价检查。反引号中的标识符（含 `_`、`.` 或大写字母，且最后一段至少三个字符）必须出现在笔记所引的某个文件中；以 `/` 或短扩展名结尾的反引号路径必须存在。它能抓住拼写错误和臆造的 API，抓不住错误的解释。

`commit:` 条目必须存在，并且是 `HEAD` 的祖先或位于 origin 的某个分支上（没有 origin 时任意远程分支均可），因为被 rebase 改写的提交可能仍留在本地，而它在托管平台上的链接已经 404（提交 `2853a01`、`34b80cd`）。

<sub>来源: [book.rs:1992-2054](../../src/book.rs#L1992-L2054) · [book.rs:594-687](../../src/book.rs#L594-L687) · [book.rs:1823-1864](../../src/book.rs#L1823-L1864) · [commit 2853a01](https://github.com/lhwzds/codocia/commit/2853a01) · [commit 34b80cd](https://github.com/lhwzds/codocia/commit/34b80cd)</sub>

## 通过共享证据发现重复

智能体写的文档里最常见的冗余，是同一机制在三个页面上各讲一遍。`repetition_warnings` 依据证据而不是文字来发现它：不同页面上的两条笔记若共享至少两个文件范围，且每处重叠都不少于较短范围的一半，就会收到警告，列出两条笔记和共享的行。只共享一个范围属于正常的交叉引用，不会报警。解决办法是只在一个页面讲解，其他页面链接过去。

<sub>来源: [book.rs:2057-2102](../../src/book.rs#L2057-L2102) · [commit 87bbfdd](https://github.com/lhwzds/codocia/commit/87bbfdd)</sub>
