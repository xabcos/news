# NEWS

NEWS 是一个按主题抓取和整理新闻的轻量服务。它调用 Responses API 的 `web_search` 与 `x_search`，将结果写入 PostgreSQL，使用 Redis 和进程内存提供公共 JSON API、Atom 源和新闻首页；登录用户还可以管理个人主题并读取个性化摘要。工作台位于 `/me/`。

## 公共 API

```text
GET /digest?topic=x&day=YYYY-MM-DD&limit=30&after=<next>
GET /insight
GET /topics
GET /days?topic=x&limit=30
GET /feed.xml?topic=ai
```

省略 `topic` 返回全部公共主题，省略 `day` 返回最近有内容的一天。`limit` 为 1–128，响应中的 `next` 游标用于继续分页。条目包含中英文标题和摘要、URL、来源、发布时间、收录时间、标签、重要性、置顶状态以及可选图片。公共响应带稳定的 `generated_at`、ETag、CORS 和短期缓存；私有响应不缓存。

## 公共主题

| ID | 方向 |
| --- | --- |
| `x` | 跨行业 AI、科技产品与产业趋势 |
| `p` | 全球热点图片、X 视觉内容与公开影像趋势 |
| `china-ai` | 中国 AI 模型、开源项目与应用生态 |
| `openai` | OpenAI、ChatGPT、Codex 与 API 官方动态 |
| `anthropic` | Anthropic、Claude 与 Claude Code 官方动态 |
| `github` | GitHub AI 项目、开发工具与开源生态 |
| `events` | 全球科技、社会与公共风险事件 |
| `cases` | 真实 AI 应用、商业流程与行业案例 |
| `codex` | Codex 使用体验、容量故障与开发者反馈 |

主题提示词同时提供中文和英文方向，抓取时要求具体事实、可靠来源、事件去重和明确的时间信息。`x` 负责跨行业概览，厂商、开源项目、故障和应用案例分别由专门主题覆盖。

## 本地运行

需要 Rust 1.94+ 和 PostgreSQL，Redis 可选：

```bash
cp .env.example .env
# 填写 DATABASE_URL、NEWS_ADMIN_EMAILS、AI_API_KEY、TMPFILE_* 等配置
cargo run
```

打开 `http://127.0.0.1:8788/` 或 `http://127.0.0.1:8788/me/`。开发环境可设置 `EMAIL_LOGIN_DEV_LINKS=true` 返回一次性登录链接；设置 `NEWS_SCHEDULER_ENABLED=false` 可暂停自动抓取。

容器运行：`make docker-up-local`。

## 配置

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | – | PostgreSQL DSN |
| `PUBLIC_BASE_URL` | – | 对外根地址；非回环地址必须使用 HTTPS |
| `NEWS_ADMIN_EMAILS` | – | 管理员邮箱，逗号分隔 |
| `AI_API_KEY` | – | Responses API 密钥；启用调度时必填 |
| `AI_BASE_URL` / `AI_DEFAULT_MODEL` | `https://api.zairouter.com` / `grok-4.6` | 新闻抓取接口和默认模型 |
| `AI_INSIGHT_MODEL` / `AI_INSIGHT_INTERVAL_MINUTES` | `gpt-6-astra` / `120` | 综合研判模型和间隔 |
| `NEWS_TIMEZONE` | `Asia/Shanghai` | 日期边界和时间窗时区 |
| `NEWS_SCHEDULER_ENABLED` | `true` | 是否启用自动抓取和研判 |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `1800` / `2` | 抓取超时和并发上限 |
| `NEWS_RETENTION_DAYS` | `128` | 新闻保留天数 |
| `TMPFILE_BASE_URL` / `TMPFILE_API_KEY` | 必填 | 图片转存接口及密钥 |
| `NEWS_IMAGE_MAX_BYTES` / `NEWS_IMAGE_TIMEOUT_SECS` | `8388608` / `300` | 图片大小和抓取超时 |
| `REDIS_ADDR` / `REDIS_PASS` / `REDIS_DB` | 空 / 空 / `1` | 邮件队列和缓存 |
| `SESSION_COOKIE_NAME` / `SESSION_TTL_DAYS` / `EMAIL_LOGIN_LINK_TTL_MINUTES` | `news_session` / `30` / `30` | 会话和登录链接有效期 |
| `YPAY_BASE_URL` / `YPAY_TENANT_CODE` / `YPAY_STORE_API_KEY` | 空 / `news` / 空 | 支付网关配置 |
| `PAYMENT_WEBHOOK_SECRET` | 空 | 支付回调签名密钥 |

## 账户和管理 API

```text
GET    /api/me                       GET    /api/me/feed
GET    /api/me/status                GET    /api/me/topics
POST   /api/me/topics                PATCH  /api/me/topics/{id}
DELETE /api/me/topics/{id}           POST   /api/me/topics/{id}/run
GET    /api/me/topics/{id}/runs      GET    /api/me/topics/{id}/items
PATCH  /api/me/items/{id}            DELETE /api/me/items/{id}

GET    /api/admin/status             GET    /api/admin/topics
POST   /api/admin/topics             PATCH  /api/admin/topics/{id}
DELETE /api/admin/topics/{id}        POST   /api/admin/topics/{id}/run
GET    /api/admin/metrics

GET    /api/plans                    GET    /api/orders
POST   /api/orders                   GET    /api/orders/{out_trade_no}
GET    /api/orders/{out_trade_no}/qr.svg
POST   /api/webhooks/payment
```

管理员管理公共主题，普通账户管理自己的主题。Free 账户可以阅读公共主题，创建个人抓取任务需要有效 Pro 套餐。Pro 订单由 YPay 微信扫码支付。

## 部署和迁移

Compose 将服务绑定到宿主机 `127.0.0.1:8793`，由 Nginx 提供域名入口和公共源站缓存：

```bash
docker compose config >/dev/null
docker compose up -d
```

启动时执行 `migrations/0001_initial.sql`，创建 NEWS 数据库、套餐目录和公共主题目录。新环境启动后，调度器会按照主题配置开始抓取。

## 验证

```bash
make release-check
```

该命令执行格式检查、测试、Clippy、全部前端 JavaScript 语法检查和 Compose 配置检查。
