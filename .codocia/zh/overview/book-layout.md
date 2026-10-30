---
title: 书放在哪里
source: #e89c60d4768f
---

`codocia init` 在仓库中创建 `.codocia/`：`book.md` 的 frontmatter 存放大纲和设置（页面、`ignore`、预算、`langs`、`translations`、固定的 `commit`），正文是引言；每条笔记是 `notes/<page>/<id>.md`；翻译放在 `<lang>/`。`repo:` 存为相对路径（`..`），所以克隆的仓库或 CI 检出无需改动即可使用。

每个命令都用 `find_book_from` 找书：从当前目录逐级向上，取第一个 `.codocia/book.md`，或旧式的外部书（同时含 `book.md` 和笔记目录的目录）。`--book` 可覆盖这一查找；自动找到或创建的 `.codocia` 不能是符号链接。

`effective_ignore` 把书目录加入忽略规则，因此笔记永远不算代码：不计入覆盖，不出现在 [[structure]]，修改笔记也不算代码变动。提交 `618468e` 把书移进仓库，让文档跟随它所描述的代码一起流转。
