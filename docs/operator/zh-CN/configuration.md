# 数据库启动配置

[English](../configuration.md#database-deadlines)

API、后台工作进程和部署命令在打开数据库之前读取期限。它们使用内置默认值，配置文件和环境变量可以覆盖，
不从数据库中的设置读取。修改后需要重启受影响的进程。部署配置文件为 `config/orch.toml`。

| `[database]` 中的键 | 默认值（毫秒） | 含义 |
|---|---|---|
| `acquire_timeout_ms` | 10000 | 获取、建立及检查池中连接的总期限，必须为正数。 |
| `request_statement_timeout_ms` | 15000 | API 语句的执行期限；零表示不设期限。 |
| `background_statement_timeout_ms` | 0 | 后台任务和维护命令的语句执行期限；零表示不设期限。 |
| `idle_transaction_timeout_ms` | 10000 | 事务内部闲置的期限；零表示不设期限。 |

例如，`ORCH_DATABASE_ACQUIRE_TIMEOUT_MS=30000` 把获取连接的期限设为三十秒。
PostgreSQL 的语句和闲置事务期限以有符号 32 位整数表示毫秒；这是原生表示范围，不是产品另设的上限。
参见 [PostgreSQL 期限设置](https://www.postgresql.org/docs/17/runtime-config-client.html#RUNTIME-CONFIG-CLIENT-STATEMENT)。
调整期限不会修复慢查询或过久持有锁的问题，仍须处理实际原因。

`[database]` 中的 `prepared_statements`（默认 512）是 API 和后台工作进程的每个数据库连接保留的已准备语句数，最久未用的先被放弃。
不再保留的语句下次运行时要重新准备：多一次往返，PostgreSQL 还要重新解析和规划，对权限读取而言这比执行本身更费时。
一次轮次的启动就要运行几百条不同的语句，最常用的五百条几乎涵盖服务运行的全部语句。每条保留的语句在其数据库会话中占用数十 KB，
两个连接池（`[api] db_pool`、`[worker] db_pool`）的每个连接都是如此。零表示不保留。`ORCH_DATABASE_PREPARED_STATEMENTS=1024`
可以覆盖它；修改后需要重启 API 和后台工作进程。

`[api]` 中的 `db_pool_warm`（默认 8）是无人请求时 API 保持打开并就绪的数据库连接数，使暂停之后打开的第一个页面由数据库已经服务过的连接作答：
这些连接保留其已准备语句及这些语句所用的内容。页面的首批读取同时在多个连接上运行。API 打开新连接时，会在后台于其上准备其他连接最常运行的语句。
超出这一数量且闲置的连接每分钟关闭一个。`db_pool_lifetime_secs`（默认 14400）是每个连接被替换前的服务时长；零表示一直保留。
`db_pool_warm = 0` 时不保留，闲置连接十分钟后关闭。修改任一项后需要重启 API。

管理 → 系统 → 详情展示 API 进程已捕获的安全运行值及其内置、文件或环境覆盖来源。后续修改不会改变对当前运行进程的标注。未报告的进程保持“尚未报告”，API 不推测其他服务的环境。
