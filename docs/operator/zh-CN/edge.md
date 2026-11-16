# HTTPS 与 WebSocket 入口

网页入口默认在 `ORCH_FRONTEND_BIND`（`127.0.0.1:5180`）接收 HTTP。可以由自己的反向代理或 CDN 提供 HTTPS，也可以启用 Orch 自带的 HTTPS 监听器。Orch 不管理 DNS 或 ACME；已有的入口可以继续管理它们。完整的地址、证书和代理配置见[英文入口手册](../edge.md)。

## 跨地区访问

使用设备信任的 HTTPS 入口，并为距离 API 较远的人启用 HTTP/2。普通 HTTP/1.1 每个源只能同时使用少量连接，脚本传输会使 API 请求排队；HTTPS 上的 HTTP/2 可以复用连接。Orch 自带的 HTTPS 监听器已启用 `http2 on`。如果由其他反向代理或 CDN 终止 TLS，应检查浏览器与该入口之间实际使用的协议。所需的 TLS 和 ALPN 支持见 [nginx HTTP/2 文档](https://nginx.org/en/docs/http/ngx_http_v2_module.html)。

对人实际打开的地址运行以下检查。如果设备尚未信任安装的签发机构，指定其公开证书：

```sh
curl --http2 --cacert /path/to/orch-ca.pem --silent --show-error --output /dev/null --write-out '%{http_version}\n' https://orch.example/
```

输出 `2` 表示实际协商了 HTTP/2。保留证书验证；这里使用的是公开的签发机构证书，不是私钥。热重载复用按内容命名的脚本；网页在应用脚本占用连接前就开始读取 Cookie 会话与首份已认证数据。稳定的 700 毫秒线路是正常线路；连接指示仅在实测延迟比这个人的平时水平更差时提醒。

HTTP/3 还需要支持 QUIC 的 nginx 构建、可达的 UDP 监听器与 `Alt-Svc` 通告。检查所用前端的 `nginx -V` 是否包含 `--with-http_v3_module`；仅有 TCP HTTPS 不能证明 QUIC 可达。详见 [HTTP/3 模块](https://nginx.org/en/docs/http/ngx_http_v3_module.html)与 [QUIC 要求](https://nginx.org/en/docs/quic.html)。外部入口也可以向浏览器提供 HTTP/3，再通过 HTTP/2 或 HTTP/1.1 访问源站。
