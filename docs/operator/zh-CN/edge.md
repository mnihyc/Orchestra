# HTTPS 与 WebSocket 入口

网页入口默认在 `ORCH_FRONTEND_BIND`（`127.0.0.1:5180`）接收 HTTP。可以由自己的反向代理或 CDN 提供 HTTPS，也可以启用 Orch 自带的 HTTPS 监听器。Orch 不管理 DNS 或 ACME；已有的入口可以继续管理它们。完整的地址、证书和代理配置见[英文入口手册](../edge.md)。

## 跨地区访问

使用设备信任的 HTTPS 入口，并为距离 API 较远的人启用 HTTP/2。普通 HTTP/1.1 每个源只能同时使用少量连接，脚本传输会使 API 请求排队；HTTPS 上的 HTTP/2 可以复用连接。Orch 自带的 HTTPS 监听器已启用 `http2 on`。如果由其他反向代理或 CDN 终止 TLS，应检查浏览器与该入口之间实际使用的协议。所需的 TLS 和 ALPN 支持见 [nginx HTTP/2 文档](https://nginx.org/en/docs/http/ngx_http_v2_module.html)。

对人实际打开的地址运行以下检查。如果设备尚未信任安装的签发机构，指定其公开证书：

```sh
curl --http2 --cacert /path/to/orch-ca.pem --silent --show-error --output /dev/null --write-out '%{http_version}\n' https://orch.example/
```

输出 `2` 表示实际协商了 HTTP/2。保留证书验证；这里使用的是公开的签发机构证书，不是私钥。热重载复用按内容命名的脚本；网页在应用脚本占用连接前就开始读取 Cookie 会话与首份已认证数据。稳定的 700 毫秒线路是正常线路；连接指示仅在实测延迟比这个人的平时水平更差时提醒。

Orch 自带的 HTTPS 还在 HTTP/2 旁启用 HTTP/3。HTTPS 覆盖文件在同一配置地址发布 TCP 与 UDP；外部防火墙也须允许该 UDP 端口。`Alt-Svc` 使用浏览器实际打开的端口，包括非标准发布端口。随附的 nginx 包含 QUIC 模块，但是否可达仍取决于浏览器到入口之间的线路。详见 [HTTP/3 模块](https://nginx.org/en/docs/http/ngx_http_v3_module.html)与 [QUIC 要求](https://nginx.org/en/docs/quic.html)。外部入口也可向浏览器提供 HTTP/3，再通过 HTTP/2 或 HTTP/1.1 访问源站。

在管理 → 设置 → **HTTPS 传输**（`edge.transport`）中调整内置 TLS 入口。默认使用 16 MiB 共享会话缓存、一天的恢复有效期、会话票据、提前安全读取、HTTP/2 和 HTTP/3、一天的协议通告有效期，以及一秒的本地重新加载检查间隔。保存后的值在事务提交后发布，并平滑重新加载 nginx；空闲检查只读取本地文件标识，不轮询数据库或发起网络请求。关闭 HTTP/3 会清除先前通告。这些值均为可调整的默认值，没有固定上限；缓存设为 0 表示关闭该缓存。外部 TLS 入口仍使用其自身的传输设置。

会话恢复可省去重复握手。nginx 管理会话票据密钥；共享缓存开启时，它会在工作进程间轮换密钥。TLS 提前数据可能被重放，因此入口传递 `Early-Data: 1`，API 会在认证或业务操作产生效果前对提前写入返回 **425**。会改变状态的登录跳转即使使用 GET，也须等待握手。握手完成后可重试；普通安全读取可以使用提前数据。详见 [nginx 会话与提前数据指令](https://nginx.org/en/docs/http/ngx_http_ssl_module.html)与 [HTTP 提前数据协议](https://www.rfc-editor.org/rfc/rfc8470.html)。设置不暴露票据密钥或证书私钥。

配置的网页地址使用 HTTP 时，管理页的安装事实会在地址旁说明此地址没有证书，跨地区访问启动较慢。这句话描述该地址，不推测另一个反向代理是否在其他地址配置了证书。
