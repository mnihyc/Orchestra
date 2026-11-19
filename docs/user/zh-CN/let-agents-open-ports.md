---
title: 让代理无需询问即可开放端口
screens: /thread/$threadId, /resources/environments/$id, /resources/templates/$id, /topic/$topicId, /project/$projectId, /approvals
keywords: 端口, 无需询问, port_place, port_host, port_link, capability.policy, ports.enabled, ports.max_open, ports.link_max_secs, requests.expiry_hours, network.allowed_hosts, 模板默认值
heading_id.选择接收范围: choose-the-audience
heading_id.允许后续开放: allow-later-openings
heading_id.把链接当作秘密: treat-links-as-secrets
heading_id.理解向外连接: understand-the-outbound-path
heading_id.在环境内的程序中读取端口: read-ports-from-a-program-inside-the-environment
---

# 让代理无需询问即可开放端口

环境的端口面板显示服务及谁可通过 Orch 到达。能看线程不具有端口权限。
查看端口允许使用服务，管理端口决定更广的开放。

## 选择接收范围

三项环境能力允许代理开放对应端口：

| 能力 | 谁能进入 | 初始策略 |
| --- | --- | --- |
| `port_place` | 指定组织、项目或话题的当前查看者 | 询问 |
| `port_host` | 能到达主机声明监听地址的人 | 拒绝 |
| `port_link` | 持有未过期链接的人 | 拒绝 |

这些是网关能力，不需驱动修改、重启或重建。模板决定哪些能力可以授予，预置模板允许
这三种，但位置策略仍约束它们。有端口管理权也不能开放策略拒绝的类型。

例如代理在 TCP 3000 启动应用，请求项目当前查看者可打开。询问策略给出允许一次、授予此
环境或拒绝。先检查项目名和范围；开放不授予文件、终端、浏览器或显示权限。

## 允许后续开放

对单个环境，在端口或能力面板授予 `port_place`。后续同类型请求无需卡片，仍记录允许
它的能力和实际接收范围。对模板以后的环境，可在允许列表内把它加入基础能力；已有环境
保留原授予。对位置中的环境，将能力策略设为允许，例如：

```json
{
  "port_place": { "decision": "allow", "approver_scope": "environment" }
}
```

之后代理可为项目查看者开放 3000，容量、入口开关和当前范围检查仍适用。
主机开放还要满足实际端口范围和本机上限。没有额外的代理端口策略或永久规则列表。

## 把链接当作秘密

提前授予 `port_link` 也允许生成链接，是高风险选择：代理得到任何持有者都能用的秘密。
策略页和卡片会说明。单次请求的仅供我使用只把链接交给决定者，告诉代理的只有链接存在及
到期时间。不要在普通诊断或公开截图中出现秘密链接。

关闭开放会结束其活动连接。收回能力阻止以后自动开放，已有开放仍按自己的期限持续到关闭。
请求过期和重复拒绝见[审批如何工作](how-approvals-work.md)。

## 理解向外连接

端口请求只控制 Orch 自己的入口。出站网络开放时，程序可连接外部隧道服务发布 3000，
无需 Orch 链接。只允许必要目的地的受限网络可关闭未列出的外连路径。网络开放时，
端口和网络控件会说明并链接出站设置。端口决定不缩小网络本来允许的范围。

## 在环境内的程序中读取端口

Orch 为已附着环境启动的进程会得到只读回环端点 `ORCH_PORTS_URL`。连接的机器还会得到
独立的 Bearer 凭据 `ORCH_PORTS_TOKEN`，属于此环境并在分离后替换；不要分享或改用个人
令牌。Orch 创建的运行时在此本地接口不需 Bearer。

```sh
orch-connector ports list --look --limit 20
orch-connector ports address 3000
orch-connector ports whois --peer 127.0.0.1:43123 --listener-port 3000 --transport tcp
```

命令输出与端口面板相同的信息和地址，不包含秘密链接。列表游标继续完整行；命令不能
创建开放、授予能力或关闭别人的连接。`port forward` 另用于转发已获准端口。

whois 使用程序接受套接字得到的精确对端，在读取 PROXY 前缀前取值；UDP 使用数据报发送者。
填写实际监听端口和传输类型。匹配返回当前两端和短期签名证明；非 Orch、已关闭或复用
的对端返回 JSON null。已知对端无法取得新证明时读取失败，应重试，不能据此认定来者未知。
用 `/ports/assertion-keys` 的公钥验证签名、签发者、自己的环境和端口范围以及时间。
公钥不需 Bearer，私钥只在服务器。

本地接口每环境允许 64 个并发查找和每滚动分钟 3600 次读取，超出返回 429 和 Retry-After。
对端回应最多 4000 字节。命令直接使用字面回环地址，不经过代理或重定向，并有等待界限。
