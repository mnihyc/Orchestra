# 人机验证

人机验证会在没有会话的人登录、创建账号、或被发送邮件之前，要求他们证明自己不是程序。它能挡住不断尝试密码、不断创建账号、
或用登录码塞满邮箱的程序。Orch 支持两个服务：

- **hCaptcha**：让人勾选一个框，或在后台运行、仅在存疑时才提问；
- **Google Cloud Fraud Defense（reCAPTCHA）**：Google 的页面仍称它为 reCAPTCHA，它在后台为人打分。

出厂时**是关闭的**。关闭时，没有任何页面从外部加载内容，也不会向任何服务发送信息，没有外网的系统照常工作。无论开关，
速率限制和错误尝试的锁定都保持不变；验证在它们之前进行，未通过验证的人不会给他输入的账号记一次错误尝试。

## 它用来做什么，不用来做什么

它保护下列操作，每一项都可以在“管理 → 登录”中去掉：

| 操作 | 位置 | 原因 |
|------|------|------|
| 登录 | `/login`，每一次尝试，包括第二步 | 第二步会再次带上密码，不要求验证的尝试就成了免费猜密码的机会 |
| 邮件登录码 | “给我发验证码” | 它会向任何账号发邮件 |
| 注册 | `/register` 与邀请页面 | 共享的邀请链接和开放注册一样公开 |
| 密码重置链接 | `/recover` | 它会向任何地址发邮件 |
| 再次发送确认链接 | “再发一次” | 它会向任何账号发邮件 |

不会要求验证的有：`/setup` 创建的第一位管理员（在此之前什么都无法开启）、通过身份提供方登录（提供方自己的登录就是证明）、
自带密钥的链接（重置链接、确认链接）、API 密钥和令牌、以及机器的登记。已经登录的人永远不会被要求验证。

用密码登录的程序（脚本、场景运行）无法通过验证，在该操作受保护时会被拒绝。它们应使用 API 密钥，API 密钥不经过登录。
`orchctl admin` 直接操作数据库，不受影响。

## 五步开启

1. **在服务那里创建密钥。** hCaptcha：为人们打开的地址（`[system] public_url`）创建站点密钥和私钥。reCAPTCHA：在 Google
   控制台为同一地址创建**基于分数**的密钥；控制台会给出网站密钥和私钥。不支持 reCAPTCHA 的复选框密钥，Google 也不推荐。
2. **让页面能访问该服务。** 网站入口的内容安全策略只允许它自己的地址；服务器在你的地址上读到的策略里还没有该服务的位置时，
   “管理”不会开启验证（它会说出缺少哪些主机）。怎么做取决于你的入口文件从哪里来：
   - **新装的，或 `nginx.conf`、`frontend.sh`、`compose.yml` 都是本版本 `orch-deploy` 包里的那一套。** 在 `.env` 中把
     `ORCH_FRONTEND_HUMAN_CHECK` 设为 `hcaptcha`、`recaptcha`，或 `recaptcha-net`（通过 `www.recaptcha.net` 使用 reCAPTCHA，
     供无法访问 `www.google.com` 的人使用），然后运行 `docker compose up -d`。变量为空时，策略与入口一直发送的完全一致。
   - **用 `./update.sh` 从更早的版本更新来的。** 更新只替换服务器、应用和 `orchctl`；`nginx.conf`、`frontend.sh`、
     `compose.yml` 保持原样，所以那个变量在这里不起作用。把 `nginx.conf` 中以 `add_header Content-Security-Policy` 开头的那一行
     改成下面对应你的服务的一行，然后运行 `docker compose restart frontend`（其他文件都不用改）：

     hCaptcha：

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://hcaptcha.com https://*.hcaptcha.com; style-src 'self' 'unsafe-inline' https://hcaptcha.com https://*.hcaptcha.com; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://hcaptcha.com https://*.hcaptcha.com; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://hcaptcha.com https://*.hcaptcha.com" always;
     ```

     Google Cloud Fraud Defense（reCAPTCHA）：

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://www.google.com/recaptcha/; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/" always;
     ```

     通过 `www.recaptcha.net` 使用 reCAPTCHA（设置“使用 recaptcha.net”）：

     ```
     add_header Content-Security-Policy "default-src 'self'; base-uri 'none'; object-src 'none'; frame-ancestors 'none'; form-action 'self'; script-src 'self' 'nonce-$request_id' 'wasm-unsafe-eval' https://www.recaptcha.net/recaptcha/ https://www.gstatic.com/recaptcha/; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; media-src 'self' blob: https:; font-src 'self'; connect-src 'self' https://www.recaptcha.net/recaptcha/; worker-src 'self'; manifest-src 'self'; frame-src 'self' https://www.recaptcha.net/recaptcha/ https://recaptcha.google.com/recaptcha/" always;
     ```

     下一个版本的 `nginx.conf` 也可以整个换上：没有 `frontend.sh` 写出的那个文件它也能启动，所以 `frontend.sh` 仍是旧的也不会出问题。
   - **你自己的前端**（apache2、CDN）设置了自己的内容安全策略：把下表中的主机加进去。浏览器会执行它收到的每一条策略，
     入口的和前端的，所以每一条都要留出位置。
3. **在“管理 → 登录 → 人机验证”中选择服务**，并填入网站密钥。对 reCAPTCHA 还要选择询问方式：*SiteVerify* 使用控制台给出的网站密钥和私钥，
   对任何密钥都有效，是用该控制台创建的密钥的选择；*CreateAssessment* 使用 Google Cloud 项目和限定为 reCAPTCHA API 的 API 密钥，
   已有项目时是 Google 推荐的方式。
4. **填入私钥**（hCaptcha 的私钥、reCAPTCHA 的私钥，或 Google 的 API 密钥）并点击**测试**。私钥以密封形式保存，之后不会再显示；
   **测试**会用一个并非真实的令牌询问服务，并告诉你是否连上服务、服务是否接受这些密钥。
5. **看一看 doctor。** `orchctl doctor --url https://你的地址` 有一行检查内容安全策略（`edge-human-check`），
   `orchctl doctor` 有一行检查验证本身（`human-check`）。doctor 读的是这个地址给出的策略，所以你自己的前端在那里增加或替换的策略能看到；
   只给部分浏览器的内容看不到（随访问者的国家、设备或 cookie 而变的策略、写在页面标记里的策略）。
   `Content-Security-Policy-Report-Only` 头不被执行，也不会被读取。

## 人们看到什么

复选框模式下，hCaptcha 自己的组件位于表单按钮上方，勾选之前按钮不可用。一个令牌只管一次尝试：每次尝试之后，无论结果如何，
框都会重置，并用一行短话说明原因。后台模式下不绘制任何内容；按下按钮即运行验证，按钮下方是服务要求页面显示的句子
（“本网站受 reCAPTCHA 保护。”或带两个链接的 hCaptcha 句子）。reCAPTCHA 的徽标被隐藏，这是 Google 在显示其句子时允许的。

组件在某人的浏览器里无法加载时（拦截器、够不到服务的网络、没有为它留位置的策略），页面会说*无法加载验证。*，不会让按钮一直不可用，
并以 `unloaded` 代替令牌发出这次尝试。无法加载的验证没有人能通过，所以服务器自己去查，不会只听信谁的一句话：
读到入口的策略里没有该服务的位置、或者连服务也问不通时，人们按“无法连接时”的设置被放行或拒绝（审计里的原因是 `edge` 或服务失败的那个词）。
入口留有位置而服务能应答时，自己浏览器加载不了组件的人仍和以前一样被要求验证，因为只凭页面一句话，程序也能这样说。

验证被拒绝时页面会说明：*未通过验证*、*已过期*，或*暂时无法连接*。服务无法连接时，人们会被**放行并记录事件**（默认），
或者在你选择**无法连接时拒绝**时看到“无法连接人机验证服务。请稍后重试，或联系管理员。”默认放行，是因为表单是唯一的入口，
服务中断或防火墙变更不应把所有人挡在门外；尝试次数的限制依然有效。

## 发送给服务的内容

打开需要验证的页面时，人们的浏览器会加载服务的脚本并与之通信，具体见服务自己的文档；服务会设置它自己的 cookie（Google 的是 `_GRECAPTCHA`）。
人们按下按钮时，Orch 向服务发送：

- 验证令牌，以及用于核验的私钥或密钥；
- **人的网络地址**，按连接和你声明的代理解析得到，使服务能发现同一个令牌在两个地方使用；
- 对 Google 的 `CreateAssessment`，还有人的浏览器描述和操作名（`orch_sign_in`、`orch_register` 等）；
- 对 hCaptcha，还有网站密钥，以防为其他密钥生成的令牌在这里使用。

除此之外不发送关于这个人的任何信息：没有姓名、地址、密码、邀请或会话。开启之前请阅读你所选服务的条款并考虑你自己的隐私义务，
并在你自己的隐私说明中写明；hCaptcha 称它是你的数据处理方，Google 的数据条款在其 Fraud Defense 页面中链接。

## 管理员能看到什么

- “管理 → 登录 → 人机验证”包含各项选择。它用到的数字（等待服务多久、一个地址最多能让本服务器发起多少次验证、以及计数的时间窗口）
  都是设置，默认值及其理由写在设置说明里。
- 审计记录中，对每个操作每小时最多有一行 `auth.human_check_admitted` 或 `auth.human_check_refused`，写明自上一行以来服务无法询问的次数，
  以及原因的词：`timeout`、`connect`、`status`、`unreadable`、`secret`（服务不接受私钥）、`setup`（服务不认识该密钥或项目）。
  设置或移除私钥时有 `auth.human_check_secret_changed`。任何一行都不含令牌或私钥。
- 服务器读取你的地址上的网站入口策略（够不到时读堆栈自己的入口），并把所见记住“查看的间隔”那么久。“管理 → 登录 → 人机验证”用一行话
  说明策略缺少哪些主机。策略里还没有该服务的位置时，选择服务、填入网站密钥、或改变加载它的地址都会被拒绝，并提示
  *网站入口的内容安全策略还不允许页面访问这个服务*；入口设置好之后，同样的选择就能通过。已经开启的选择保持原样；
  入口在它下面被改成组件无法加载时，人们同样按上面的办法被放行或拒绝，所以一份错的策略不会把表单关死。
- `orchctl doctor` 报告最近一天的事件，以及私钥是否已保存。

## 内容安全策略必须允许的主机

`ORCH_FRONTEND_HUMAN_CHECK` 会写入下列主机，它们来自各服务自己的文档：

| 服务 | `script-src` | `frame-src`（另含 `'self'`） | `connect-src` | `style-src` |
|------|--------------|------------------------------|---------------|-------------|
| `hcaptcha` | `https://hcaptcha.com https://*.hcaptcha.com` | 相同 | 相同 | 相同 |
| `recaptcha` | `https://www.google.com/recaptcha/ https://www.gstatic.com/recaptcha/` | `https://www.google.com/recaptcha/ https://recaptcha.google.com/recaptcha/` | `https://www.google.com/recaptcha/` | 无 |
| `recaptcha-net` | `https://www.recaptcha.net/recaptcha/ https://www.gstatic.com/recaptcha/` | `https://www.recaptcha.net/recaptcha/ https://recaptcha.google.com/recaptcha/` | `https://www.recaptcha.net/recaptcha/` | 无 |

hCaptcha 要求不要写出具体子域名。该策略对每个页面生效，而不只是登录页面，因为单页应用的页面会保留首个加载页面的策略；
但脚本只会加到需要验证的页面上。

## 关闭

在“管理 → 登录 → 人机验证”中选择**关闭**。它的密钥和选择都会保留。如果没有人能通过验证（服务无法连接而你选择了拒绝），
在主机上运行：

```
docker compose exec api orchctl admin human-check off
```

它会以系统身份关闭验证并留下审计记录。把 `ORCH_FRONTEND_HUMAN_CHECK` 清空并运行 `docker compose up -d`，即可把这些主机从内容安全策略中去掉。
