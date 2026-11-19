---
title: 阅读视图与详情
screens: /, /workbench, /org/$slug, /org/$slug/messages, /org/$slug/settings/$section, /project/$projectId, /project/$projectId/settings, /topic/$topicId, /thread/$threadId, /doc/$documentId, /documents/$nodeId, /document-folder/$folderId, /chat/$chatId, /place-chat/$nodeId, /inbox, /approvals, /everything, /search, /shared, /usage, /u/$username, /resources/providers, /resources/providers/$id, /resources/agents, /resources/agents/$id, /resources/environments, /resources/environments/$id, /resources/hosts, /resources/hosts/$id, /resources/machines, /resources/machines/$id, /resources/templates, /resources/templates/$id, /resources/mcp, /resources/mcp/$id, /resources/mounts, /resources/mounts/$id, /resources/skills, /resources/skills/$id, /settings, /admin, /help
keywords: 视图, 标签, 导航, 资源详情, 卡片, 菜单, 键盘, 手机, 长名称, 设置, 权限
heading_id.打开页面: opening-a-screen
heading_id.视图和导航: views-and-navigation
heading_id.卡片和菜单: cards-and-menus
heading_id.数值和设置: values-and-settings
heading_id.阅读文档和文件: reading-documents-and-files
heading_id.查找名称和命令: finding-names-and-commands
heading_keywords.finding-names-and-commands: 查找, 模式, 全部
---

# 阅读视图与详情

## 打开页面

按名称打开会话或资源。第一页显示你有权读取的信息。
关闭的导航视图或文档分区，在你打开它时才加载内容。
其他地方显示过一个名称，并不意味着你有权访问它。权限改变后，已打开的项目也可能变得不可用。

加载时显示骨架行。读取失败时，保留错误和可用的下一步操作。
未知的数值仍显示为未知，不会当作零。

## 视图和导航

手机和桌面上的标签、视图控件使用相同的名称。
每个标签都显示名称，当前标签也一样。标签栏较窄时，长名称会缩短显示。
手机始终显示当前标签；“查找标签”用于定位其他已打开的视图。
同名视图会用类型或你可见的路径区分。仍有重复时，再加视图编号。
项目本身的名称不会改变。
个人空间和聊天会在页面标题与标签中标明用途。
页面重新加载时，标签会保留已读取的名称。

**资源导航**打开资源类型列表，**资源**打开资源概览。
**位置文档**跟随当前位置，也可以固定到一个位置。
**系统模板**是管理页面中的模板目录。**提供方用量**打开该提供方的报告；
用量统计、性能和请求切换报告内容。

## 卡片和菜单

将指针停在人员标签上、用键盘聚焦它，或按下它，都可以阅读人员卡片。
资源和引用标签用同样的方式打开详情。
紧凑行中的长名称会缩短显示；悬停或聚焦可读到完整名称。

卡片、帮助、菜单、选择列表和日期选择器都保持在屏幕内。
内容较长时，在卡片或列表内部滚动。手机也使用同样的小帮助卡片。
Escape 关闭最上层。关闭保留打开的卡片后，焦点回到它的控件。

即使下面还有多行信息，行菜单仍与第一行名称对齐。
支持右键或键盘菜单键的地方，会打开相同的操作。
不可用的操作保留原因；打开卡片不会增加你的权限。

## 数值和设置

设置的名称旁保留实际数值和来源，菜单与问号归属于这个名称。
紧凑偏好控件将说明放在提示中，悬停或聚焦即可阅读。
打开问号可阅读更多说明，并进入链接到的文章。

铃铛显示已确认的通知级别。聊天的“关闭”与位置的“关闭”分别生效。
修改须等待回答。讨论通知修改被拒绝时，原级别保留，错误在铃铛旁打开。
Escape 关闭错误卡片，并把焦点还给铃铛。

## 阅读文档和文件

文档标题、正文标题和文件内容是作者的数据。重复的文字会原样保留。
文档的附件按钮会标明文件要加入的文档。
任务复选框带有任务名称。代码块提供语言选择和复制整块内容的操作。
来自外部地址的音频、视频，只有在你选择播放后才加载。

文件名、地址、镜像引用和服务名称保留准确拼写，可按显示的值复制。
显示器的名称与菜单保持在一起；辅助命令保留自己的复制操作。

## 查找名称和命令

按 `Ctrl+K`（macOS 上用 `Command+K`）打开查找。
选择**全部**可清除模式符号，同时查找名称、命令、设置和帮助。
模式按钮会在输入框中写入对应符号，也可以直接输入该符号。
选择**对话**或输入 `/` 来查找说过的内容。
