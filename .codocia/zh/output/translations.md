---
title: 不会悄悄漂移的翻译
source: #93de5ab8c281
---

Codocia 从不翻译；它只记录每份翻译是根据哪个源文本做的。书的翻译包括 `<lang>/book.md`（标题、页面标题、引言）和每条笔记对应的 `<lang>/<page>/<id>.md`，frontmatter 中带 `source: #hash`；哈希覆盖源笔记的标题和正文，不含证据，所以重新固定代码不会使译文失效。仓库文件的翻译对写在 `book.md` 中，形如 `README_CN.md: README.md #hash`。

翻译缺失、从未同步、源哈希已变，或所译笔记的源已被删除时，`check` 报错。之后 `parity` 比较机械结构：标题数量、代码块（忽略注释）、ASCII 行内代码或外部 URL 不一致时发出警告，因为这些在每种语言中都必须相同。`accept zh`（或 `zh/book`、`zh/page/id`、`README_CN.md`）会运行 `sync`，写入当前的源哈希；它不评判译文质量。
