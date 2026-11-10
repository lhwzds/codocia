---
title: 把一页写成 SKILL.md
source: #8b0840aef86e
---

加上 `--skill-page <page>` 后，`build` 还会写出 `<out>/skill/<id>/SKILL.md`，让 skrun 和其他智能体可以把这一页当作技能存储和查找。写任何文件之前先检查页面，未知的 slug 直接失败。id 是仓库目录名，只保留 `[a-z0-9._-]`。这个文件和其他页面一样记录在 `.codocia-build` 中，所以之后不带该参数的构建会删除它，`remove_empty_parents` 再删掉因此变空的目录。

frontmatter 每个键占一行（`name`、`description`、`version`、`tags: [<id>]`），因为 skrun 用逐行解析器而不是 YAML 读取它。`skill_scalar` 让每个值对两种解析器都安全：没有 `"`，首尾没有引号，没有 `\\`。description 取第一条笔记第一段的第一句。version 依次取自 `Cargo.toml`、`pyproject.toml` 或 `package.json`；TOML 读取器拿不准时（遇到第一个 `"""` 或 `'''`）什么也不返回，绝不返回别的表的版本，最后的默认值是 `0.1.0`。源码链接是带 12 位提交号的永久链接：技能库会隐藏含 40 位十六进制串的行。
