# NEWS 设计说明

> 最后更新：2026-09-07

NEWS 是一个单实例、轻量的 AI 新闻聚合服务。它把「公共新闻」和「个人主题」放在同一个进程里，但只保留一层清晰的账户边界：所有账户使用 `/me/` 工作台，按角色管理个人或公共主题。没有团队、组织、邀请或多级角色；公共接口保持开放，账户摘要接口按会话隔离。

## 1. 产品边界

每个有效邮箱都可以通过一次性邮件链接登录。登录后：

- 普通用户只能读写自己创建的主题、运行记录和条目；登录后首页展示公共主题和自己的启用主题，个人条目不会进入公共 digest。
- 管理员可以管理所有公共主题、触发抓取和维护公共条目。
- 管理员在工作台中只管理公共主题，不提供个人主题、套餐或订单入口；管理员接口不会读取个人主题。

主题的默认编辑面保持很小：名称、关注方向、启用状态和抓取间隔。个人主题默认每天 06:00–24:00 运行；服务端保留模型、时间窗等字段供管理员维护，个人用户不需要理解这些成本参数。

## 2. 套餐与限制

限制集中在 `src/auth.rs` 的两个常量中，不在数据库里复制一套易漂移的权限系统：

| 套餐 | 主题数 | 最短间隔 | 每日运行 | 有效期 | 价格 |
| --- | ---: | ---: | ---: | ---: | ---: |
| `free` | 1 | 240 分钟 | 12 次 | 长期 | ¥0 |
| `pro` | 8 | 60 分钟 | 48 次 | 31 天 | ¥99 |

用户的 Pro 状态只由 `users.pro_until > now()` 判定。付款后从当前仍有效的到期时间继续顺延 31 天；过期后不删除已有主题，只保留最早创建的 1 个主题运行，其余主题仍可查看、编辑和删除，但会标记为暂停，删除后自动顺延下一个。这样不会因为续费或降级引入额外状态迁移。

每日运行次数按用户汇总，而不是按主题单独计算。手动运行和定时运行共用同一条准入检查：主题启用、最短间隔和当日上限。调度器在计划切换后下一次排程立即使用新限制。

## 3. 数据模型

完整 schema（账户、主题、套餐与支付表）统一在 `migrations/0001_initial.sql`；项目按全新部署设计，启动时只需执行这一份初始 migration。

### users

```text
id, email, status, role ('user' | 'admin'), pro_until, created_at
```

`NEWS_ADMIN_EMAILS` 中的邮箱首次登录时得到 `admin` 角色；其他合法邮箱自动创建为 `user`。管理员角色一旦写入数据库便保持不变，避免配置短暂变化导致正在使用的后台突然失去权限。

### topics

保留原有的抓取配置，并新增：

```text
owner_user_id NULL    = 管理员维护的公共主题
owner_user_id 非 NULL = 对应账户的个人主题
```

个人主题由服务端生成 32 位紧凑 UUID，避免与 `ai`、`openai` 等公共 slug 冲突，也不需要让普通用户设计 URL。所有个人 CRUD 查询都带 `owner_user_id = current_user.id`；管理员接口明确检查 `role` 后只读取 `owner_user_id IS NULL` 的公共主题。

### plans / orders / payment_events

`plans` 只有 `free` 和 `pro` 两行，订单保存创建时的金额和天数，避免后续改目录影响旧订单。`orders.expires_at` 默认是创建后 30 分钟，过期的待支付订单在下次访问或创建订单时标记为 `closed`。已签名的延迟 `paid` 回调仍可将其入账，避免用户已付款却因页面超时丢失权益。

`payment_events.event_key = paid:{out_trade_no}` 是唯一键；回调事务同时锁订单和用户，写入事件、延长 `pro_until`、把订单改成 `paid`，重复回调直接返回当前到期时间并再次唤醒可运行主题。创建支付订单先提交本地 pending 记录，再调用 YPay；`payment_started_at` 只作为两分钟短租约，避免慢请求占用数据库。不确定的网络失败会保留租约；租约过期时关闭旧单并生成新单，旧单仍接受已签名的延迟回调，避免进程重启留下永远无法重新生成二维码的订单。

## 4. 路由边界

### 公共读取

```text
GET /digest?topic=&day=&limit=&after=
GET /insight
GET /topics
GET /days?topic=&limit=
GET /feed.xml?topic=
```

这些路由只从 `owner_user_id IS NULL` 且启用的主题读取，开放 CORS，带短时 `ETag` 缓存。Redis 和内存快照只服务公共数据；个人运行完成时不会刷新公共快照。全部主题的首页 `/digest` 按 `limit` 预渲染缓存正文与 ETag，`/days` 与 `/insight` 的响应体也常驻内存，随写入、日切和清理一起失效；`/insight` 只返回正文引用到的来源并保留其原始 `index`。

分页是 keyset 游标而不是偏移量：`next` 编码最后一条的显示顺序键（`pinned`、`published_at`、`first_seen_at`、`id`），下一页从严格晚于该键的条目开始，因此两页之间的新增或隐藏不会让条目被跳过或重复；置顶变化会改变键本身，属于重新排序，由页面刷新收敛。主题 `revision` 只用于判断后台刷新是否需要重置已加载分页，不编入游标。公共路径在内存快照上按键二分定位，个人 feed 在 SQL 里用同样的行比较；快照读取时按该键归一排序，`items` 查询也以 `id DESC` 收尾，保证顺序确定。

### 账户与主题

```text
POST /api/auth/email/start
POST /api/auth/email/consume
POST /api/auth/logout
GET  /api/me
GET  /api/me/feed?topic=&day=&limit=&after=
GET  /api/me/status

GET/POST     /api/me/topics
PATCH/DELETE /api/me/topics/{id}
POST         /api/me/topics/{id}/run
GET          /api/me/topics/{id}/runs
GET          /api/me/topics/{id}/items
PATCH/DELETE /api/me/items/{id}
```

`/api/me/*` 始终按当前会话过滤。管理员专用的同构路由是 `/api/admin/*`，每个入口都调用 `require_admin`。公共主题没有面向普通用户的写入口；外部读取走只读 digest，账户首页则在私有响应中复用这些公共内容。

`GET /api/me` 返回账户、当前套餐限制以及 `topic_count`、`runs_today`，工作台只需一次请求即可渲染用量。个人主题内容接口省略日期时选该主题最近有内容的一天。

登录后的首页通过 `GET /api/me/feed` 读取启用的公共主题和当前账户的个人主题。它一次返回 `topics`、`days` 和与公共 `/digest` 同形的 `digest`，支持 `topic`、`day`、`limit`、`after` 查询并返回同样的 `next` 游标；查询只允许 `owner_user_id IS NULL` 或 `owner_user_id = current_user.id`，响应 `no-store`，不使用公共 DigestCache/Redis，避免个人内容污染或进入公共缓存。

## 5. YPay 微信支付

支付选择固定为 YPay 的 store-token + webhook 模式，不在 NEWS 内实现微信 SDK：

1. 普通账户在工作台的「套餐与用量」面板中升级或续费 Pro，服务端只接受 `plan_code=pro`，并从数据库目录读取固定 ¥99 / 31 天。
2. NEWS 调用 `POST /t/{tenant}/store/orders/create`，传入用户 ID、订单号、金额和 `pay_method=native`。
3. YPay 返回 `code_url`；NEWS 保存支付响应，并由 `/api/orders/{out_trade_no}/qr.svg` 在同源生成二维码。
4. YPay 向 `/api/webhooks/payment` 发送 HMAC-SHA256 签名的 `paid` 回调。验签成功后事务性地开通或顺延 Pro；已有有效期时从原到期时间继续顺延 31 天。

YPay 租户后台应配置：

```text
auth_mode         = store_token
fulfillment_mode  = webhook
fulfillment_url   = https://news.xairouter.com/api/webhooks/payment
```

`fulfillment_secret` 与 NEWS 的 `PAYMENT_WEBHOOK_SECRET` 相同。NEWS 启动时要求 `YPAY_BASE_URL`、`YPAY_STORE_API_KEY`、`PAYMENT_WEBHOOK_SECRET` 三项要么全部配置、要么全部为空；支付未配置时普通账户仍可正常使用 Free。

## 6. 调度与数据流

调度器每 30 秒认领到期主题，使用进程内 `in_flight` 集合保证单实例下同一主题不重复运行，并用一个共享信号量把定时和手动抓取都限制在 `NEWS_MAX_CONCURRENT_RUNS` 内；数据库的 `FOR UPDATE SKIP LOCKED` 负责认领。运行结束后更新 `runs` 和主题状态，再为下一次运行计算时间。

失败采用短指数退避，但不会低于当前套餐的最短间隔；达到每日上限则等到服务时区的下一天。公共主题沿用原有 15 分钟基础退避，个人主题使用 Free/Pro 的 120/60 分钟下限。

抓取流程仍是：

```text
Responses API(web_search + x_search)
        ↓
校验 URL / 发布时间 / 长度
        ↓
指纹与标题去重
        ↓
PostgreSQL 事务写入 items + runs
        ↓
公共主题刷新 Redis / 内存快照
```

个人条目不构建公共快照，仍按新闻保留策略保存在 PostgreSQL，工作台按主题读取最近内容。

综合研判是独立于新闻主题的公共栏目。调度器默认每两小时读取六个公共主题最近 24 小时的条目，同时读取最近 7 天 `ai_insights` 中每天最后一份成功研判，再使用 `AI_INSIGHT_MODEL`（默认 `gpt-6-astra`）生成中英文短标题、自然的趋势分析、商业与机遇、就业与技能变化、风险预警和观察方向。趋势判断资料不足时，模型按模型能力按需使用 `web_search`，Grok 模型另可使用 `x_search` 补充公开数据，并把实际使用的来源保存在 `research_sources`；新闻与检索来源都可在页面直接打开。新闻抓取会在有可靠结果时保留一张热点照片及图注，研判从带图新闻中选择一张主图，优先避开最近 32 小时已经使用过的配图，候选耗尽时使用最久未用的一张；没有可靠图片就保持纯文字。首页在 `𝐀𝐋𝐋` 右侧展示 `𝐀𝐈`。运行记录保留 3 天，AI 研判历史保留 32 天，新闻条目默认保留 128 天，均由服务内维护任务自动清理。

页面卡片的相对时间使用 `first_seen_at`（NEWS 收录时间）；`published_at` 继续作为 API 的来源发布时间保留。很多来源只能确定日期，直接把日期归一到 UTC 午夜会让刚收录的内容看起来过时。

## 7. 前端

`/me/` 是唯一工作台页面。所有账户共用登录、主题列表、基础编辑器、内容展示和请求生命周期；前端按登录角色选择 `/api/me/*` 或 `/api/admin/*`，服务端继续独立执行角色与归属检查。管理员没有个人主题和套餐功能。

默认页面集中展示主题和最近内容。基础编辑只包含名称、关注方向、启用状态和抓取间隔；管理员的公共 ID、公开说明、模型与排程参数收进「发布与抓取设置」，运行记录按需展开，内容保留置顶、隐藏与删除操作。个人账户的套餐、用量、订单和扫码支付在「套餐与用量」面板中。

进入工作台时先确认会话，再显示对应界面，验证期间不显示登录表单或启动遮罩。首页中英文和工作台均不显示加载占位文案；空数据、失败原因与重试操作仍按实际结果展示。

前端沿用原生 JavaScript 模块：`app.js` 管理会话与主题，`account.js` 管理个人套餐与支付，`ui.js` 提供请求和对话框等公共交互。字段直接使用当前服务端契约。切换账户或退出时清空旧账户数据并中止请求和轮询，切换主题时忽略旧响应；后台刷新不改写正在编辑的表单，并通过运行时间变化同步抓取结果。操作成功后的刷新失败单独处理，支付结果支持重试同步套餐与用量。

公共首页的首屏只有一轮网络往返。打开根路径时 `index.html` 用 `<link rel="preload" as="fetch">` 提前发出 `/digest?limit=12`、`/days` 和 `/topics`，`app.js` 启动后并行取用；带 `topic`/`day` 的深链和已登录账户仍是一轮请求，只是用不上这三份预加载。是否登录由登录时一同写入的可读 cookie `news_role` 决定：匿名访客不再探测 `/api/me`，个人账户直接请求 `/api/me/feed`，遇到 401/403 时清掉该 cookie 回退到公共内容。

每个主题首屏 12 条，组尾放一个空的哨兵元素，进入视口即用该组的 `next` 游标续拉下一页。前一天只在当天所有主题都读完后加载：底部加载器先按顺序补齐读者快速滑过、仍有分页的主题，插入到视口上方时依赖浏览器滚动锚定，不支持锚定的浏览器由脚本修正滚动位置。每个主题的 feed revision 随可见条目变化递增；后台刷新发现版本没变就保留已加载内容，发现变化就用新首屏替换该主题并重置游标。这样隐藏和新置顶会及时收敛，客户端不需要合并或猜测排序；续页统一按 id 去重追加。

`pages/styles.css` 提供全站基础样式和新闻首页样式，`pages/me/styles.css` 集中管理工作台布局与控件，沿用既有颜色和排版，并支持手机与深色模式。旧管理员页面已删除，不保留跳转入口。

## 8. 配置与部署

必须配置：

```text
DATABASE_URL
PUBLIC_BASE_URL
NEWS_ADMIN_EMAILS
AI_API_KEY                 # 调度开启时
AI_DEFAULT_MODEL=grok-4.6
AI_INSIGHT_MODEL=gpt-6-astra
AI_INSIGHT_INTERVAL_MINUTES=120
NEWS_FETCH_TIMEOUT_SECS=1800
```

生产登录邮件仍通过 `~/xabcos/mail` 的 Redis 契约发送。支付启用时再配置：

```text
YPAY_BASE_URL
YPAY_TENANT_CODE=news
YPAY_STORE_API_KEY
YPAY_PAY_METHOD=native
PAYMENT_WEBHOOK_SECRET
```

宿主机 nginx 使用 `deploy/nginx/news.xairouter.com.conf`：`/api` 与 `/me` 直通，其余公共路径按服务的 `Cache-Control` 做源站微缓存。该文件必须被主配置的 `http {}` 块 include 才会生效，安装与 `nginx -T` 验证步骤和 Cloudflare 侧的 Cache Rule 见 README「反向代理与边缘缓存」。

启动会自动执行 migrations。当前项目按全新部署设计，不保留旧白名单变量或旧订单结构的兼容分支。发布前运行：

```bash
make release-check
```

它覆盖 Rust 格式、单元测试、Clippy、全部前端模块的 JavaScript 语法检查和 Docker Compose 配置检查。
