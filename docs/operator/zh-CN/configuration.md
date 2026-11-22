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

管理 → 系统 → 详情展示 API 进程已捕获的安全运行值及其内置、文件或环境覆盖来源。后续修改不会改变对当前运行进程的标注。未报告的进程保持“尚未报告”，API 不推测其他服务的环境。
