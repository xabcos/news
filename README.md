# NEWS

一个轻量的新闻聚合服务。NEWS 按主题调用 `grok-4.6`（Responses API，`web_search` + `x_search`）检索最新报道，按天写入 PostgreSQL，热数据放 Redis 和进程内存，对外提供无鉴权的公共 JSON API、Atom 源和一个简洁的聚合页；登录用户还可读取包含公共与个人主题的个性化摘要。每个有效邮箱都可以登录统一工作台 `/me/`；普通账户管理个人主题，管理员管理公共主题。

生产地址：`https://news.xairouter.com`，工作台 `/me/`。设计说明见 [`DESIGN.md`](DESIGN.md)。

套餐只有两档：`free` 最多 1 个主题、最短 120 分钟；`pro` 最多 8 个主题、最短 60 分钟，微信支付 ¥99 开通 31 天（每日运行上限分别为 12 次和 48 次）。

## 公共 API

```text
GET https://news.xairouter.com/digest?topic=ai&day=2026-09-03&limit=30
GET https://news.xairouter.com/insight
GET https://news.xairouter.com/topics
GET https://news.xairouter.com/days?topic=ai&limit=30
GET https://news.xairouter.com/feed.xml?topic=ai
```

- `topic` 省略时返回全部启用主题；`day` 省略时返回最近一个有内容的日期（响应里的 `day` 字段说明是哪一天）；`limit` 1–128，默认 30，按主题各自截断。
- `/insight` 返回基于最近 24 小时 NEWS 内容的 `𝐀𝐈` 综合研判，默认每 2 小时更新一次，并读取最近 7 天内每天最后一份成功研判作为趋势分析上下文；有可靠来源时返回一张热点主图、图注和来源链接，优先避开最近 32 小时用过的配图，候选耗尽时使用最久未用的一张。
- 每条包含 `title/summary`（中文）、`title_en/summary_en`、`url`、`source`、`published_at`、`first_seen_at`、`tags`、`importance`（1–5）、`pinned`。
- 全部开放 CORS，带 `ETag` 与 `Cache-Control: public, max-age=60, stale-while-revalidate=300`，浏览器和 Cloudflare 都能直接复用；错误响应同样带 CORS 且 `no-store`。

```json
{"day":"2026-09-03","timezone":"Asia/Shanghai","generated_at":"…","topics":[{"id":"ai","name":"AI 行业动态","name_en":"AI Industry","description":"…","latest_day":"2026-09-03","updated_at":"…","total":14,"items":[{"id":"…","title":"…","summary":"…","title_en":"…","summary_en":"…","url":"https://…","source":"Reuters","published_at":"…","first_seen_at":"…","tags":["OpenAI"],"importance":5,"pinned":false}]}]}
```

## 工作方式

- **调度**：每个主题有间隔（分钟）、时间窗（整点）、星期和每日上限，全部按 `NEWS_TIMEZONE`（默认 Asia/Shanghai）解释。调度器每 30 秒认领到期主题并异步抓取，一次抓取通常 40–130 秒；失败按 15 分钟起指数退避，不超过正常间隔。
- **增量**：每次把最近两天已收录的标题和链接交给模型，只要新增或有实质进展的条目；服务端再按规范化 URL 指纹和标题相似度去重。当天的摘要随时间累积，而不是每小时整页替换。
- **时间显示**：API 同时保留来源发布时间 `published_at` 和 NEWS 收录时间 `first_seen_at`；页面卡片的相对时间以 `first_seen_at` 为准，避免来源只提供日期时被误显示成数小时前。
- **三层数据**：读路径 内存 → Redis → PostgreSQL，逐级回填；写路径 PostgreSQL 事务提交 → Redis → 原子替换内存快照。启动时预热今天与昨天，第一次请求就命中内存。每天 00:00 淘汰前天的内存快照；Redis key 48 小时过期；新闻条目默认保留 128 天（可用 `NEWS_RETENTION_DAYS` 调整）。`REDIS_ADDR` 为空时自动降级为内存 + PostgreSQL。
- **自动清理**：服务启动时和之后每小时分批清理数据；抓取运行记录保留 3 天，AI 研判历史保留 32 天，新闻条目按 `NEWS_RETENTION_DAYS` 保留，避免依赖外部定时任务。
- **单实例**：内存快照只在本进程内失效，多实例部署会长期返回旧数据。
- **成本**：每次抓取约 $0.06–0.10（`usage.cost_in_usd_ticks`，1 tick = 1e-10 USD）。工作台的管理员运行记录按次显示 tokens、搜索次数和成本，以及今日合计。
- **个人主题**：登录后首页保留公共主题，并追加当前账户启用的个人主题；`/me/` 负责管理个人主题。个人主题不会进入公共 digest；管理员主题仍是公共内容源。
- **个人排程**：个人主题默认每天 06:00–24:00 运行，用户只需设置抓取间隔；Free/Pro 的最短间隔分别为 120/60 分钟。
- **访问统计**：公共首页通过同源 `stats.js` 向 Stats 发送匿名页面访问；查询参数、锚点、主题切换和无限下拉不会重复计数。统计失败不影响页面使用，工作台不接入统计。
- **页面广播**：公共首页复用 Broadcast 的 `feed_news?mode=sequence`；没有有效消息时通知条自动隐藏。

## 本地运行

需要 Rust 1.94+ 与 PostgreSQL；Redis 仅用于生产邮件队列和跨重启缓存，本地可不配。

```bash
cp .env.example .env   # 填写 DATABASE_URL、NEWS_ADMIN_EMAILS、AI_API_KEY
cargo run
```

打开 `http://127.0.0.1:8788/`（公开页）、`http://127.0.0.1:8788/me/`（统一工作台）。`EMAIL_LOGIN_DEV_LINKS=true` 时登录响应直接给出一次性链接，只允许回环地址。`NEWS_SCHEDULER_ENABLED=false` 可以只读运行、不花钱。

完全容器化：`make docker-up-local`。

## 配置

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | – | PostgreSQL DSN，启动时自动执行 migrations |
| `PUBLIC_BASE_URL` | – | 对外根地址，用于登录链接与 Origin 校验；非回环必须 HTTPS |
| `NEWS_ADMIN_EMAILS` | – | 管理员邮箱，逗号分隔；其他有效邮箱自动使用 Free |
| `AI_API_KEY` | – | 调用模型的密钥，只存在于环境变量 |
| `AI_BASE_URL` / `AI_DEFAULT_MODEL` | `https://api.zairouter.com` / `grok-4.6` | 新闻抓取接口与默认模型 |
| `NEWS_TIMEZONE` | `Asia/Shanghai` | 天边界与时间窗 |
| `NEWS_SCHEDULER_ENABLED` | `true` | `false` 时只提供页面与 API |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `1800` / `2` | 新闻抓取、综合研判与 AI Agent 共用超时，默认为 30 分钟 |
| `AI_INSIGHT_MODEL` / `AI_INSIGHT_INTERVAL_MINUTES` | `gpt-6-astra` / `120` | 综合研判模型与更新间隔 |
| `NEWS_RETENTION_DAYS` | `128` | 新闻条目保留天数，超过期限自动清理 |
| `REDIS_ADDR` / `REDIS_PASS` / `REDIS_DB` | 空 / 空 / `1` | 邮件队列与缓存 |
| `SESSION_COOKIE_NAME` / `SESSION_TTL_DAYS` / `EMAIL_LOGIN_LINK_TTL_MINUTES` | `news_session` / `30` / `30` | |
| `EMAIL_STREAM_PRIORITY` / `EMAIL_OID` / `EMAIL_REPLY_TO` | `important` / `0` / 空 | 与 `~/xabcos/mail` 的契约一致 |
| `YPAY_BASE_URL` | 空 | YPay 网关地址 |
| `YPAY_TENANT_CODE` | `news` | YPay store-token 租户代码 |
| `YPAY_STORE_API_KEY` | 空 | YPay 租户 store token |
| `YPAY_PAY_METHOD` | `native` | 微信 Native 支付 |
| `PAYMENT_WEBHOOK_SECRET` | 空 | YPay 回调 HMAC 密钥；启用支付时与上面三项一起配置 |

## 账户与管理 API

登录链接从 `/me/` 开始。接口只接受同源浏览器请求；`/api/admin/*` 额外要求管理员角色：

```text
GET    /api/me
GET    /api/me/feed?topic=&day=&limit=
GET    /api/me/status
GET    /api/me/topics             POST   /api/me/topics
PATCH  /api/me/topics/{id}        DELETE /api/me/topics/{id}
POST   /api/me/topics/{id}/run
GET    /api/me/topics/{id}/runs?limit=50
GET    /api/me/topics/{id}/items?day=YYYY-MM-DD
PATCH  /api/me/items/{id}         DELETE /api/me/items/{id}

GET    /api/admin/status          GET    /api/admin/topics
POST   /api/admin/topics          PATCH  /api/admin/topics/{id}
DELETE /api/admin/topics/{id}     POST   /api/admin/topics/{id}/run
GET    /api/admin/topics/{id}/runs?limit=50
GET    /api/admin/topics/{id}/items?day=YYYY-MM-DD
PATCH  /api/admin/items/{id}      DELETE /api/admin/items/{id}

GET    /api/plans                 GET    /api/orders
POST   /api/orders                GET    /api/orders/{out_trade_no}
GET    /api/orders/{out_trade_no}/qr.svg
POST   /api/webhooks/payment
```

个人主题的 `GET /api/me/topics/{id}/items` 在省略 `day` 时返回该主题最近有内容的一天；传入 `day=YYYY-MM-DD` 可查看指定日期。

登录后首页使用 `GET /api/me/feed` 读取公共主题和当前账户启用的个人主题。响应包含 `topics`（主题卡片）、`days`（所选主题有内容的日期，最新优先）以及与公共 `/digest` 同形的 `digest`；`topic`、`day`、`limit` 的语义与公共接口一致。该响应按会话隔离并始终 `no-store`，不会进入公共 Redis 或内存缓存；个人条目仍只会在当前账户的响应中出现。

登录发送频率限制与 broadcast 相同：同一邮箱 60 秒内不重发，10 分钟内最多 3 次，全站 60 秒内最多 120 次。

Pro 订单固定为 ¥99 / 31 天，通过 YPay 的 store-token 接口创建微信 Native 订单。YPay 租户使用 webhook fulfillment，回调地址为 `https://news.xairouter.com/api/webhooks/payment`，并将租户密钥与 `PAYMENT_WEBHOOK_SECRET` 保持一致；服务端只接受固定 Pro 金额，回调验签后延长 `users.pro_until`。

`/api/plans` 返回套餐列表和 `checkout_enabled`；未配置完整 YPay 三项时，个人页会保留套餐说明但不显示可点击的升级按钮。

## 部署

服务监听容器内 `8788`，`docker-compose.yml` 只绑定宿主机 `127.0.0.1:8793`；把 `news.xairouter.com` 的反向代理 upstream 指向它。`.env`（`0600`）提供 `NEWS_ADMIN_EMAILS`、`AI_API_KEY`、SMTP `EMAIL_PASS` 以及启用支付所需的 YPay 三项配置。YPay 侧需先创建与 `YPAY_TENANT_CODE` 对应的 NEWS 租户，并把 webhook fulfillment URL 指向上面的回调地址：

```bash
cd /home/ywgx/1/news
printf 'NEWS_ADMIN_EMAILS=admin@example.com\nAI_API_KEY=…\nEMAIL_PASS=…\nYPAY_BASE_URL=…\nYPAY_TENANT_CODE=news\nYPAY_STORE_API_KEY=…\nPAYMENT_WEBHOOK_SECRET=…\n' > .env && chmod 0600 .env
docker-compose config >/dev/null
docker-compose up -d
```

数据库会按顺序自动执行 `migrations/` 中的 migration；当前版本按全新部署设计，请使用空数据库，或重建 `postgres_data` volume 后再启动。

CI 与 broadcast 相同：`quality-gate.yml` 执行 `make release-check`；`build-deploy.yml` 在 `main` push 后构建并推送 `rust`、`<date>-rust`、`<version>-rust` 标签，commit message 含 `prod` 或手动触发时部署生产；`manual-deploy-prod.yml` 直接部署当前镜像。

## 验证

```bash
make release-check   # fmt、test、clippy、全部 JS 语法、compose 配置
```

单元测试覆盖工作台角色边界、调度计算、URL 规范化与指纹、标题相似度、条目校验、真实 Responses 返回样例（`tests/fixtures/`）的解析、ETag/304。
