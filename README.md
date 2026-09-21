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

省略 `topic` 返回全部公共主题，省略 `day` 返回最近有内容的一天。`limit` 为 1–128，响应中的 `next` 游标用于继续分页。条目包含中英文标题和摘要、URL、来源、发布时间、收录时间、标签、重要性、置顶状态以及可选图片。`/insight` 返回最新的 XAI 日报公开稿；公众号和 X Articles 中文稿只通过管理员工作台读取。公共响应带稳定的 `generated_at`、ETag、CORS 和短期缓存；私有响应不缓存。

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

主题提示词同时提供中文和英文方向，抓取时要求具体事实、可靠来源、事件去重和明确的时间信息。`x` 负责跨行业概览，厂商、开源项目、故障和应用案例分别由专门主题覆盖。`gossip` 收录有原始来源的 AI 圈爆料、回应和用户反馈，明确区分事实与传闻；迁移会为新环境补充该栏目，保留已有栏目配置。

首页顶部的 `AI` 标签进入每日 **XAI 日报**。默认每天北京时间 **06:30 开始采集**：先补采所有启用的公共栏目（含 `gossip` 和图片新闻 `p`），再生成日报。新闻使用固定的前24小时 `[昨日06:30, 今日06:30)`；趋势分析使用同一截止点前7天的新闻与完整去重统计。数量比较采用本期24小时与此前6日日均，传闻、图片帖及发布时间不明的内容不计入已确认新闻数量。首见时间不冒充发布时间。

日报依次呈现「今日热点 → AI 圈热议 → 图片新闻 → 趋势分析 → 关键数据 → 后续观察及其他分析 → 参考来源」，没有内容的分组自动省略。公众号 Markdown 稿与 X Articles 稿从同一组结构化事实排版；X 的「复制排版稿」同时提供 HTML 和纯文本，使用紧凑列表，避免空段落。图片单独上传，后台提供封面/正文顺序、图说、署名和原文链接。

配图优先取自图片新闻栏目，模型实际查看最多12张候选图，按相关性和画面吸引力选择1张封面、至多2张正文图；近7天用过的封面降权，按内容哈希或原始地址去重。资料图注明背景，无合适图片不凑数。

图片下载与转存共用3个并发名额，同一批次的相同原图复用转存地址。单张图片默认最多30秒（`NEWS_IMAGE_TIMEOUT_SECS`），整批最多90秒；普通新闻的图片失败时保留正文，图片栏目仍要求有效配图。图片续期同步更新内容版本，日报页面通过 ETag 识别更新。模型输入保留事实、日期与引用索引，双语展示字段和图片内部元数据留在存储层。

管理员可重新生成最近一期已截止日报；重试与补跑保持相同时间窗口，成功补采的栏目不重复调用。采集与生成共用并发额度，协调任务不占用额度，单并发也可运行。失败10分钟后重试，成功后替换同一天的旧稿；期间保留上一份可用稿。自动调度由 `NEWS_SCHEDULER_ENABLED` 控制，时间由 `NEWS_BRIEFING_TIME` 配置。

补采仍计入公共主题的每日运行上限；额度不足时会显示失败原因，不会绕过成本限制。Compose 同样读取调度开关与并发设置；若 `.env` 中 `NEWS_SCHEDULER_ENABLED=false`，容器也不会自动采集。

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
| `AI_INSIGHT_MODEL` | `gpt-6-astra` | 支持图片输入的 XAI 日报模型 |
| `NEWS_BRIEFING_TIME` | `06:30` | 按 `NEWS_TIMEZONE` 开始日报补采，完成后生成 |
| `NEWS_TIMEZONE` | `Asia/Shanghai` | 日期边界和时间窗时区 |
| `NEWS_SCHEDULER_ENABLED` | `true` | 是否启用自动抓取和每日 XAI 日报 |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `1800` / `2` | 抓取超时和并发上限 |
| `NEWS_RETENTION_DAYS` | `128` | 新闻保留天数 |
| `TMPFILE_BASE_URL` / `TMPFILE_API_KEY` | 必填 | 图片转存接口及密钥 |
| `NEWS_IMAGE_MAX_BYTES` / `NEWS_IMAGE_TIMEOUT_SECS` | `8388608` / `30` | 图片大小和单张下载、上传的总超时 |
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
GET    /api/admin/insight            POST   /api/admin/insight

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

启动时按顺序执行 `migrations/` 中的迁移，创建 NEWS 数据库、套餐目录和公共主题目录。新环境启动后，调度器按照主题配置抓取，每天06:30补采并生成 XAI 日报；重启会补跑最近一期尚未完成的日报。

## 验证

```bash
make release-check
```

该命令执行格式检查、测试、Clippy、全部前端 JavaScript 语法检查和 Compose 配置检查。

日报的 PostgreSQL 集成测试单独运行：在本地开发环境变量之外设置 `NEWS_DAILY_TEST_DATABASE_URL`，指向可清空的独立测试库（名称以 `news_daily_test` 开头），然后运行 `cargo test --locked postgres_daily_pipeline -- --ignored`。测试使用本地模拟接口，验证时间边界、7天去重统计、图片输入、单并发补采、失败续跑、日报发布，以及图片续期、地址复用、并发上限和超时取消，不调用真实 AI 服务。
