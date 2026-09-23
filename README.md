# NEWS

NEWS 按主题采集、去重并整理中英文新闻，提供新闻首页、Atom 订阅和每日 XAI 日报。工作台位于 `/me/`，管理员管理公共栏目和日报稿件，Pro 用户管理个人主题。

## 公共栏目

| 名称 | ID | 关注方向 |
| --- | --- | --- |
| 𝐗 | `x` | 全球 AI、科技产品、硬件、支付、创业与产业趋势 |
| 𝑷 | `p` | AI、科技、军事、财经热点图片和公开视觉内容 |
| 𝑹𝒆𝒂𝒅𝒔 | `reads` | 值得读完的原创文章与公开长帖，原文、全文翻译与本站精读 |
| 𝑪 | `china-ai` | 中国模型、开发平台、开源与应用生态 |
| 𝑶 | `overseas-ai` | OpenAI、Anthropic、Grok、Gemini、Mistral 等海外 LLM 的产品、研究与商业动态 |
| 𝑮𝒐𝒔𝒔𝒊𝒑 | `gossip` | AI 圈爆料、回应、公开热议与网络安全事件 |
| 𝑮𝒊𝒕𝑯𝒖𝒃 | `github` | GitHub、Rust、AI 开源项目与开发者生态 |
| 𝑬𝒗𝒆𝒏𝒕𝒔 | `events` | 全球经济、科技、社会、国际局势与公共风险 |
| 𝑪𝒂𝒔𝒆𝒔 | `cases` | 真实 AI 应用、业务流程与行业案例 |
| 𝑰𝒔𝒔𝒖𝒆𝒔 | `issues` | Codex、Grok、Claude 的使用反馈、故障与解决方案 |

栏目方向、关注来源和采集参数定义在 [初始化文件](migrations/0001_schema.sql)，可通过管理员工作台调整。采集要求具体事实、原文链接、明确时间和事件去重；Gossip 明确区分已确认消息、爆料、回应与推测。

## Reads

`/reads` 展示长期保留的精选文章。默认每 64 分钟全天采集，覆盖欧美、日本、德国、法国等地区，暂不采集中文互联网媒体。30 个精选 RSS/Atom 来源与 X、全网搜索共同发现线索，优先近 72 小时新作，以近 30 天仍有价值的文章补充。品质优先，期望每天收录 1–24 篇，每天最多 24 篇、每轮最多 4 篇，不设最低数量，没有合格内容时允许不收录。只处理初筛选中的候选，再逐篇审阅正文；初筛为空或失败时不自动追加订阅文章，不为凑数放宽标准。候选池只保留 30 天的 URL、标题、来源、日期和重试状态，永久去重沿用文章指纹；单源失败独立跳过，临时失败间隔 6 小时、最多尝试 3 次。

订阅源最多 4 路并发，每源最多读取 2 MiB、保留 30 个候选；每轮按新鲜度和来源轮选最多 160 个候选给编辑，其中为近 30 天文章预留至少 40 个位置；最终最多处理 32 个候选，优先新作，预留至多 8 个较早优文补选位置，新作不足时再扩大补选。检索最多 5 分钟，单篇处理最多 10 分钟（含全文翻译），仍受整轮时间预算约束，并为已成功的文章预留提交时间，复用现有调度并发限制。原站正文与 X 官方 oEmbed 请求限定公开 HTTPS、逐跳校验并固定 DNS 地址，限时、限体积，不携带凭据；过短、不完整、付费或登录内容跳过。

通过品质审核的公开完整文章统一生成精读与全文，不再依赖授权白名单或特定许可标记。先展示中英文本站精读，再以细分隔线接上全文：中文页展示中文译文；英文原作在英文页保留原文，其他语言在英文页展示英文译文。翻译在采集时分段生成、校验段落完整性并入库；原文结构、代码、链接目标不交给模型改写。翻译失败时仅发布合格精读，不发布部分译文。

正文图片复用 `TMPFILE_BASE_URL` / `TMPFILE_API_KEY` 转存，支持 PNG、JPEG、WebP、GIF、AVIF、BMP；保留并翻译原图注，代码及图片本身不翻译。每篇最多处理 64 张图片，遵守现有图片体积、超时和共享并发限制；图片原站请求同样逐跳固定公开 DNS。页面只加载图片服务器返回的未过期地址，使用懒加载；暂时转存失败的图片保留文字说明。已有每小时图片维护任务按到期索引最多处理 24 篇（整批图片最多 90 秒），覆盖历史文章，失败保留旧地址并稍后重试，无新增服务。

精读依据已读取正文撰写独立解读，保留原文链接、署名和正文哈希；无法支持有价值精读时跳过。搜索摘要不会冒充全文。详情页 `/reads/{uuid}` 直接输出 HTML，带独立标题、canonical、语言版本、Article 数据和分享信息；`/sitemap.xml` 提供分片站点地图，Atom 中的 Reads 链接指向本站详情。

文章与正文在一个事务中发布，跨日期永久去重。Reads 不受新闻保留天数影响，仍可在工作台隐藏、置顶或删除；隐藏或停用后详情和站点地图均不再公开。沿用现有调度、并发限制、运行统计和匿名访问统计，无额外定时服务。

列表通过专用部分索引按置顶、收录时间排序。“继续阅读”从最近 200 篇可见的其他文章中优先选择标签相关的 3 篇，再读取卡片信息，限制历史积累带来的查询开销。

Reads 默认按收录时间连续展示，不按天分组，每页 12 篇；“热读”跨日期选择最近 30 天收录的文章，按 `ln(1 + 累计PV) × 2^(-收录天数 / 7)` 排序，同一来源域名最多 2 篇，最多展示 24 篇。只对真实阅读数大于零的文章排名，有几篇展示几篇，无结果时显示空状态并链接到最新文章，始终保留热读入口与选中状态。它不是近一周阅读次数排行榜。列表的阅读数紧跟日期，详情页放在标题右侧，直接使用现有 Stats 采集响应，不额外请求、不重复计数；中英文合并到同一文章路径。

后台每 64 分钟通过 Stats `POST /stats/{public_key}/pages` 分批查询最近 30 天文章（最多 1500 篇，每批 100 个 URL），一次事务更新缓存阅读数与榜单，失败保留上一份结果。服务端页面只读本地数据库；隐藏、禁用或超过 30 天的文章不会进入热读，旧文章详情仍获取当前阅读数。部署时先发布支持批量查询的 Stats，再发布 News。

采集、正文、页面与热读逻辑集中在 `src/reads/`；交互复用 `pages/public.js`。临时下线只需禁用 `reads` 主题，调度和统计刷新会停止；彻底移除时再删除 Reads 模块及接线、资源、`read_candidates` / `read_articles` 表和专属索引，使用新的迁移清理，不修改已执行的迁移。

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

服务绑定宿主机 `127.0.0.1:8793`。将 [Nginx 配置](deploy/nginx/xabcnews.com.conf) 加入网关的 `http` 配置，为 `xabcnews.com` 提供入口。应用启动时按顺序执行数据库迁移，创建表结构、Free / Pro 套餐和 10 个公共栏目；调度器按栏目配置采集，并生成最近一期已截止日报。

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
GET /reads?page=1&lang=en
GET /reads?view=hot&lang=en
GET /reads/{uuid}
GET /sitemap.xml
```

省略 `topic` 读取全部公共栏目，省略 `day` 读取最近有内容的日期。`limit` 为 1–128，`next` 为后续分页游标。公共响应支持 ETag 和短期缓存，私有响应不缓存。

账户与个人主题使用 `/api/auth/*`、`/api/me/*`；公共主题管理使用 `/api/admin/topics`；日报状态和生成使用 `GET /api/admin/insight`、`POST /api/admin/insight`。套餐、订单和支付回调分别位于 `/api/plans`、`/api/orders`、`/api/webhooks/payment`。Free 用户阅读公共内容，Pro 用户可创建最多 4 个个人主题。

## 验证

```bash
make release-check
NEWS_DAILY_TEST_DATABASE_URL=postgres://.../news_daily_test_check cargo test --locked postgres_daily_pipeline -- --ignored
NEWS_DAILY_TEST_DATABASE_URL=postgres://.../news_daily_test_check NEWS_PAYMENT_TEST_DATABASE_URL=postgres://.../news_payment_audit_check NEWS_READS_TEST_DATABASE_URL=postgres://.../news_reads_test_check make integration-test
```

`release-check` 执行格式检查、单元测试、Clippy、前端语法和 Compose 配置检查。`integration-test` 使用三个独立空库，名称分别以 `news_daily_test`、`news_payment_audit_` 和 `news_reads_test` 开头，并需要本地测试配置。模拟接口覆盖日报与支付流程，以及 Reads 的全文翻译与失败回退、用量累计、永久去重、并发发布上限、事务回滚、长期保留、服务端正文、语言版本、缓存、隐藏和删除。CI 自动执行。
