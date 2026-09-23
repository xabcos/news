# NEWS

NEWS 按主题采集、去重并整理中英文新闻，提供新闻首页、Atom 订阅和每日 XAI 日报。工作台位于 `/me/`，管理员管理公共栏目和日报稿件，Pro 用户管理个人主题。

## 公共栏目

| 名称 | ID | 关注方向 |
| --- | --- | --- |
| 𝐗 | `x` | 全球 AI、科技产品、硬件、支付、创业与产业趋势 |
| 𝑷 | `p` | AI、科技、军事、财经热点图片和公开视觉内容 |
| 𝑪 | `china-ai` | 中国模型、开发平台、开源与应用生态 |
| 𝑶 | `overseas-ai` | OpenAI、Anthropic、Grok、Gemini、Mistral 等海外 LLM 的产品、研究与商业动态 |
| 𝑮𝒐𝒔𝒔𝒊𝒑 | `gossip` | AI 圈爆料、回应、公开热议与网络安全事件 |
| 𝑮𝒊𝒕𝑯𝒖𝒃 | `github` | GitHub、Rust、AI 开源项目与开发者生态 |
| 𝑬𝒗𝒆𝒏𝒕𝒔 | `events` | 全球经济、科技、社会、国际局势与公共风险 |
| 𝑪𝒂𝒔𝒆𝒔 | `cases` | 真实 AI 应用、业务流程与行业案例 |
| 𝑰𝒔𝒔𝒖𝒆𝒔 | `issues` | Codex、Grok、Claude 的使用反馈、故障与解决方案 |

栏目方向、关注来源和采集参数定义在 [初始化文件](migrations/0001_schema.sql)，可通过管理员工作台调整。采集要求具体事实、原文链接、明确时间和事件去重；Gossip 明确区分已确认消息、爆料、回应与推测。

## XAI 日报

首页的 `AI` 入口展示 XAI 日报。默认每天北京时间 06:00 补采启用的公共栏目，然后生成前 24 小时的新闻与最近 7 天的趋势。新闻窗口固定为 `[昨日06:00, 今日06:00)`，手动生成与自动重试共用这一窗口。

统计范围为 **`x`、`p`、`china-ai`、`overseas-ai`、`gossip`、`cases`**。本期数量、栏目与标签分布、7 天统计和此前 6 日日均均使用这一范围：先筛选启用的公共栏目、有效发布时间和可见条目，再按原文 URL 指纹去重。统计表示报道收录数量，Gossip 被收录不代表其说法已证实。

日报保存结构化事实、引用、图片选择和统计快照。网页、公众号 Markdown、X Articles 纯文本及 HTML 从这些内容生成，共用主标题。页面依次呈现热点、AI 圈热议、趋势、关键数据、后续观察和参考来源；空分组省略。关键数据来自有引用的价格、成本、效率或性能事实。

配图优先使用 `p` 栏目。模型查看最多 12 张候选图，选择至多 1 张封面和 2 张正文图，按内容哈希或原始地址去重。网页配图优先请求 AVIF，原样转存并自动续期。日报生成时单独向原站请求 JPEG、PNG、WebP 或 GIF，完整解码校验后，以临时静态 JPEG 直接输入模型；无法获取兼容版本的图片不参与模型选图。发布内容继续使用原图地址。公众号稿可直接使用图片链接，X 稿提供图片顺序、图说、署名和原文链接。

## 运行与部署

需要 Rust 1.94+、PostgreSQL 和图片转存服务。Redis 为公共缓存和邮件队列提供支持。

```bash
cp .env.example .env
# 填写数据库、管理员邮箱、AI 和图片转存配置
cargo run
```

默认访问 `http://127.0.0.1:8788/`。开发环境可用 `EMAIL_LOGIN_DEV_LINKS=true` 返回登录链接；`NEWS_SCHEDULER_ENABLED=false` 暂停自动采集。本地容器运行使用 `make docker-up-local`。

生产部署：

```bash
docker compose config >/dev/null
docker compose up -d
```

服务绑定宿主机 `127.0.0.1:8793`。将 [Nginx 配置](deploy/nginx/xabcnews.com.conf) 加入网关的 `http` 配置，为 `xabcnews.com` 提供入口。应用启动时执行单个初始化迁移，创建表结构、Free / Pro 套餐和 9 个公共栏目；调度器按栏目配置采集，并生成最近一期已截止日报。

## 配置

| 变量 | 默认 | 作用 |
| --- | --- | --- |
| `DATABASE_URL` | 必填 | PostgreSQL 连接地址 |
| `PUBLIC_BASE_URL` | 必填 | 对外地址，生产环境使用 HTTPS |
| `NEWS_ADMIN_EMAILS` | 必填 | 管理员邮箱，逗号分隔 |
| `AI_API_KEY` | 启用采集时必填 | Responses API 密钥 |
| `AI_BASE_URL` / `AI_DEFAULT_MODEL` | `https://api.zairouter.com` / `grok-4.7` | 采集接口与新建主题的模型 |
| `AI_INSIGHT_MODEL` | `gpt-6-astra` | 支持图片输入的日报模型 |
| `NEWS_TIMEZONE` / `NEWS_BRIEFING_TIME` | `Asia/Shanghai` / `06:00` | 日期边界和日报截止时间 |
| `NEWS_SCHEDULER_ENABLED` | `true` | 自动采集与日报调度 |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `1800` / `2` | 请求超时与采集、生成共用并发数 |
| `NEWS_RETENTION_DAYS` | `128` | 新闻保留天数 |
| `TMPFILE_BASE_URL` / `TMPFILE_API_KEY` | 必填 | 图片转存接口与密钥 |
| `NEWS_IMAGE_MAX_BYTES` / `NEWS_IMAGE_TIMEOUT_SECS` | `8388608` / `30` | 单张图片大小与下载、上传总超时 |
| `REDIS_ADDR` / `REDIS_PASS` / `REDIS_DB` | 空 / 空 / `1` | 缓存和邮件队列 |
| `SESSION_COOKIE_NAME` / `SESSION_TTL_DAYS` | `news_session` / `30` | 登录会话 |
| `EMAIL_LOGIN_LINK_TTL_MINUTES` | `30` | 一次性登录链接有效期 |
| `YPAY_BASE_URL` / `YPAY_TENANT_CODE` / `YPAY_STORE_API_KEY` | 空 / `news` / 空 | YPay 微信扫码支付 |
| `PAYMENT_WEBHOOK_SECRET` | 空 | 支付回调签名密钥 |

## API

```text
GET /digest?topic=x&day=YYYY-MM-DD&limit=30&after=<next>
GET /topics
GET /days?topic=x&limit=30
GET /feed.xml?topic=overseas-ai
GET /insight
```

省略 `topic` 读取全部公共栏目，省略 `day` 读取最近有内容的日期。`limit` 为 1–128，`next` 为后续分页游标。公共响应支持 ETag 和短期缓存，私有响应不缓存。

账户与个人主题使用 `/api/auth/*`、`/api/me/*`；公共主题管理使用 `/api/admin/topics`；日报状态和生成使用 `GET /api/admin/insight`、`POST /api/admin/insight`。套餐、订单和支付回调分别位于 `/api/plans`、`/api/orders`、`/api/webhooks/payment`。Free 用户阅读公共内容，Pro 用户可创建最多 4 个个人主题。

## 验证

```bash
make release-check
NEWS_DAILY_TEST_DATABASE_URL=postgres://.../news_daily_test_check cargo test --locked postgres_daily_pipeline -- --ignored
NEWS_DAILY_TEST_DATABASE_URL=postgres://.../news_daily_test_check NEWS_PAYMENT_TEST_DATABASE_URL=postgres://.../news_payment_audit_check make integration-test
```

`release-check` 执行格式检查、单元测试、Clippy、前端语法和 Compose 配置检查。`integration-test` 使用两个独立空库，名称分别以 `news_daily_test` 和 `news_payment_audit_` 开头，并需要本地测试配置。它通过模拟接口验证初始化、首次访问、统计范围、时间边界、去重、单并发补采、失败续跑、日报落库重试、图片续期及支付幂等。CI 自动执行两组检查。
