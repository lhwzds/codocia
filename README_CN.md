# Codocia

[English](README.md) | 中文

把智能体对代码库的理解，变成小而有源码依据的文档，并在文档落后于代码时提醒你。

Codocia 是面向 AI 编程智能体（Claude Code、Codex、OpenCode 等）及其审阅者的笔记本和编译器。它从不调用模型。智能体阅读代码，保存**带证据的笔记**；Codocia 把每个引用的行范围固定到内容哈希，生成代码结构图，执行字数预算，让翻译与源文本保持同步，并把笔记编译成几页易读的 Markdown。

## 安装

```bash
brew install lhwzds/tap/codocia
cargo install codocia
npm install -g codocia      # 下载发布的二进制文件
```

## 快速开始

```bash
cd your-repo
codocia init                      # 创建 .codocia/
codocia skill                     # 交给智能体的工作流程
codocia note add --page overview --id request-flow --title "How a request flows" \
  -e src/server.rs:120-180 --body 'The `Server` accepts …'
codocia check                     # 证据是否最新？符号是否真实？预算？
codocia build                     # 写出 docs/
```

提交 `.codocia/` 和 `docs/`。之后任何克隆仓库的人以及 CI 都可以运行 `codocia check`；笔记写成后若所引代码发生变化，它就会失败。

```
your-repo/
├── .codocia/
│   ├── book.md               # 标题、页面、预算、语言、翻译文件
│   ├── notes/<page>/<id>.md  # 每条笔记一个文件，附带证据
│   └── zh/…                  # 翻译，各自固定到英文源文本
└── docs/                     # 生成：README.md、NN-page.md、all.md、structure.md、zh/
```

## 为什么用带证据的笔记

- **默认简洁。** 页数和字数预算随代码规模变化（最多 8 页，每页 900 词，每条笔记 180 词；一个拉丁词或两个 CJK 字符计为一个词）。一条笔记回答一个问题；一页讲一个功能领域，而不是一个文件。
- **有据可查。** 每条笔记都引用 `path:start-end`、`commit:<sha>`、`pr:<n>` 或 `issue:<n>`，文件证据固定到所引行的哈希。
- **易于维护。** 代码移动时，`check` 会跟随。所引代码变化时，`check` 会失败，直到有人用 `accept` 审阅笔记；它会展示变动前后的代码行，并拒绝已经覆盖另一段代码的范围。
- **结构是生成的，不是写的。** 每次构建都会加入一个从代码计算出的 `structure` 页面：功能区、各区自带的描述、被导入最多的文件、导入依赖，以及哪些页面讲解了各个区域。
- **双语不漂移。** 笔记的翻译和 `README_CN.md` 这类文件的翻译都记录了所依据的源文本；源文本变化后 `check` 失败，代码、链接或标题不一致时发出警告。
- **对智能体友好。** 纯 Markdown 文件，每条笔记一个文件，多个智能体可以并行编写不同页面。

## 命令

| 命令 | 用途 |
| --- | --- |
| `init` | 为当前仓库创建 `.codocia/` |
| `status [--map] [--history] [--area dir]` | 页面、预算和覆盖；生成的结构；设计历史 |
| `note add/list/rm` | 保存（`--replace` 保留未给出的字段）、列出、删除笔记 |
| `check [--page p]` | 证据、链接、符号、预算、翻译；跟随移动的代码 |
| `accept <target>` | 记录一次审阅：笔记（`page/id`）或翻译（`zh/page/id`、`zh`、`README_CN.md`） |
| `build [--out dir]` | 写出 `docs/`（或 `book.md` 中 `out:` 指定的目录）以及 `docs/<lang>/`，并把书移到当前提交 |
| `skill` | 打印智能体工作流程（[SKILL.md](SKILL.md)） |

## 在 CI 中

```yaml
- run: cargo install codocia
- run: codocia check
```

## 示例

RestLoop 的文档用 Codocia 构建，有英文和中文版：
[docs/](https://github.com/lhwzds/restloop/tree/main/docs) 和
[docs/zh/](https://github.com/lhwzds/restloop/tree/main/docs/zh)。

## 许可证

MIT
