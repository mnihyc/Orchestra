---
title: 把文档写成文件
screens: /doc/$documentId, /documents/$nodeId, /document-folder/$folderId
keywords: markdown, orch markdown, html, word, docx, 文本, 粘贴, 导入, 导出, 压缩包, zip, 相对链接, 图片, 折叠, 分栏, 标注, 颜色, 下划线, 分页, 代理, edit_document, download_shared, 检出, 提交, 文档, 文件
heading_id.粘贴: paste
heading_id.导入文件: import-files
heading_id.word-文件: word-files
heading_id.代理怎么做: what-agents-do
heading_id.文件保留不了什么: what-a-file-cannot-keep
heading_keywords.orch-markdown: 标注, 折叠, 分栏, 颜色, 下划线, 视频, 音频, 分页, 公式, 标签
heading_keywords.paste: 剪贴板, 富文本, 页面, 图片, 提示
heading_keywords.import-files: 文件夹, zip, 链接, 引用, 图片, notion, lark, 报告, 缺失
heading_keywords.word-files: docx, 标题, 表格, 图片, 样式
heading_keywords.what-agents-do: 文件, 格式, seq, 块, 地址, 批量, 导入, 检出, 提交, 评论
heading_keywords.what-a-file-cannot-keep: 丢失, 颜色, 对齐, 合并, 列宽, 空白
---

# 把文档写成文件

Orch 读取 Markdown、HTML、Word 和纯文本，写出 Markdown 和 HTML。粘贴、导入和代理共用同一个读取器，
所以同一段文字在哪里读都得到同样的块。

## Orch Markdown

Orch Markdown 是 GitHub 的 Markdown，加上公式（`$x^2$`，或单独成行的 `$$`）、图表（`mermaid` 代码块）、
以提示块写成的标注，以及一小组标签：

````markdown
> [!WARNING]
> 标注：NOTE、TIP、WARNING 或 CAUTION。

<details>
<summary>折叠块</summary>

它下面的块。

</details>

<columns>

<column ratio="2">

较宽的一栏。

</column>

<column>

较窄的一栏。

</column>

</columns>

带<span color="red" background="amber">颜色</span>和<u>下划线</u>的文字。

<video src="https://example.org/clip.mp4">按地址引用的视频</video>

<page-break/>
````

每个标签单独占一行，前后各空一行。颜色有 red、orange、amber、lime、green、teal、cyan、blue、indigo、
purple、pink 和 grey。栏的 `ratio` 是权重；省略则各栏等宽。其他标签按文本保留，没有配对的标签也一样。
来自 Notion、飞书和 GitHub 的文本按原样读取：Notion 缩进的子块、`gray` 和 `red_bg` 颜色，以及它的 `<toggle>`，都能识别。

写成 Markdown 再读回的文档，块保持不变，只有块的标识、作者与历史、评论不在其中。

## 粘贴

把 Markdown 或富文本页面粘贴到文档，会按光标位置变成它所表示的块。粘贴页面中的内嵌图片会成为文档文件。
简短的提示会说明哪些内容按文字保留或被省略。代码和表格单元格中的粘贴按原文进入。

## 导入文件

导入接受 Markdown、HTML、Word 和文本文件，或它们的压缩包。文件夹会变成文档文件夹。
一个文件指向另一个文件的链接会变成指向对应文档的引用；文件引用的图片和文件会成为该文档的文件。
指向不到的链接或图片按文字保留，结果会说明是哪些。网络上的图片保持为链接，Orch 不会去获取。

## Word 文件

`.docx` 文件会先转成 HTML，再像普通页面一样读取。标题、列表、表格、粗体、斜体、下划线、链接和图片都会带过来。
Orch 不认识的样式按普通文本读取，结果会提示。

## 代理怎么做

代理可以列出以“可查看”共享的空文档文件夹。添加或导入文档需要“可编辑”。

代理先把想写的文档写成文件，再导入：`edit_document` 用 `file` 和 `format`（markdown、html、docx 或 text）
代替 `markdown`。文件旁边的图片会一起带入。

要修改文档时，代理先用 `download_shared` 复制它：文件开头写着文档和它的 `seq`。
用自己的工具改好文件后，用 `rewrite` 加 `file` 提交。只有改动的部分会变：原样写出的块保持不动，
文字有改动的块仍是同一个块，所以它上面的评论和指向它的链接都不会失效。
`replace` 对一段文字也是这样；`block_replace`、`block_insert_after`、`block_delete` 和 `block_move_after`
按地址修改单个块（`#3`，或第三块下的第二块 `#3.2`）。`batch` 一次完成多处这样的修改。
`import` 把一个目录或 zip 中的文件导入文档文件夹。
导入目录或 zip 后，用相同的 `path` 和 `file` 再次调用 `import` 可查看扫描和导入进度。
加上 `cancel: true` 可停止导入，已导入的文档会保留；加上 `resume: true` 可继续，权限会重新检查。
同一服务进程最多同时扫描或转换两项代理导入。达到限制时，等待一项完成后重试，或先取消已有任务。

## 文件保留不了什么

Orch Markdown 会保留折叠标题、单元格的跨行跨列和颜色、块的颜色与对齐、表格列宽、空白段落和嵌套结构。
普通 Markdown 无法表达的排版会放进可读文字周围的 `<!-- orch:block ... -->` 注释中。
编辑导出文件时请保留这些注释；它们记录排版，不记录块标识或作者。其他 Markdown 查看器会隐藏注释并显示正文。
带格式的图片说明写在注释的 Markdown 字段中，普通内容不需要注释。
含美元符号、换行或表格竖线的行内公式会用 span 包住可读的代码形式；请保留公式的 `orch-math` 属性。
如果修改了可见的代码内容，系统会保留修改后的文字。

重新导入会生成新的块标识，文件不包含作者、历史和讨论。检出后提交回原文档时，系统会匹配原有块，
保留匹配块的标识、评论和链接。
