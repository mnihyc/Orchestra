---
title: 管理账号和个人资料
screens: /login, /register, /recover, /reset/$token, /verify-email/$token, /invite/$token, /settings, /settings/profile, /settings/your-page, /settings/security, /settings/tokens, /settings/exports, /settings/interface, /settings/notifications, /settings/usage, /u/$username
keywords: 账号, 个人资料, 登录, 密码, 恢复, 邮箱, 验证器, 会话, API 令牌, 导出, 界面, 个人设置
heading_id.登录和恢复访问: sign-in-and-recover-access
heading_id.设置个人资料: set-your-profile
heading_id.保护登录: protect-sign-in
heading_id.保留个人选择: keep-personal-choices
---

# 管理账号和个人资料

## 登录和恢复访问

使用安装提供的登录方法。注册和邀请遵循该安装的策略。通过收到的链接验证邮箱；
链接过期或已使用时，页面会说明下一步。验证邮箱不会替代理登录模型提供方。

无法登录时使用恢复访问。重置链接只用一次且会过期，按表单操作，不要分享链接。
无法完成第二步验证时，请联系安装管理员按流程重置。

## 设置个人资料

设置 → 个人资料编辑姓名及资料。你的页面保存别人打开公开资料时看到的文字和固定项。
他们只能看到当前有权读取的固定工作和活动；固定不授予目标访问权。

## 保护登录

设置 → 安全管理登录方式、第二步验证、恢复码和已登录浏览器。新方式完成设置后才可
依赖。把恢复码保存在自己控制的位置。某个浏览器不应再访问账号时，从会话列表将它退出。

在令牌中管理 API 令牌。填写名称、权限、位置和有效期，创建时复制秘密值。
以后列表只显示元数据，不能再读秘密值。不再使用时撤销。令牌受你当前权限限制，
不具有仅浏览器可用的管理员权限。

## 保留个人选择

界面设置语言、主题和快捷键偏好。帮助 → 快捷键是按键编辑器，关闭单键快捷键后仍可
打开。通知设置路线和例外，用量显示自己的消耗，导出保留账号可访问的导出作业和下载。
失败作业会保留原因，不表示已准备好下载。

这些选择不改变他人的布局或位置设置。继承值和消耗见
[设置与消耗](settings-and-usage.md)。
