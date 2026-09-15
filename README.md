# NEWS

一个轻量的新闻聚合服务。NEWS 按主题调用 `grok-4.6`（Responses API，`web_search` + `x_search`）检索最新报道，按天写入 PostgreSQL，热数据放 Redis 和进程内存，对外提供无鉴权的公共 JSON API、Atom 源和一个简洁的聚合页；登录用户还可读取包含公共与个人主题的个性化摘要。每个有效邮箱都可以登录统一工作台 `/me/`；普通账户管理个人主题，管理员管理公共主题。

生产地址：`https://xabcnews.com`，工作台 `/me/`。设计说明见 [`DESIGN.md`](DESIGN.md)。

套餐只有两档：`free` 最多 1 个主题、最短 512 分钟；`pro` 最多 4 个主题、最短 64 分钟，微信支付 ¥99 开通 31 天（每日运行上限分别为 12 次和 48 次）。

## 公共 API

```text
GET https://xabcnews.com/digest?topic=ai&day=2026-09-03&limit=30&after=<next>
GET https://xabcnews.com/insight
GET https://xabcnews.com/topics
GET https://xabcnews.com/days?topic=ai&limit=30
GET https://xabcnews.com/feed.xml?topic=ai
```

- `topic` 省略时返回全部启用主题；`day` 省略时返回最近一个有内容的日期（响应里的 `day` 字段说明是哪一天）；`limit` 1–128，默认 32，每个主题各截一页。每个主题带 `next` 游标，把它作为 `after` 并指定同一个 `topic`、`day` 传回即可取下一页；`total` 始终是当天全部条数，读完时 `next` 为 `null`。游标按显示顺序（置顶、发布时间、收录时间、id）定位，两页之间新增或隐藏的条目不会被跳过或重复；翻页期间的置顶变化属于重新排序，被置顶的条目会在页面下一次刷新时出现在顶部，取消置顶的条目可能再次出现，调用方按 `id` 去重即可。`after` 无效或没有同时给 `topic` 时返回 400。
- `/insight` 返回基于最近 24 小时 NEWS 内容的 `𝐀𝐈` 综合研判，默认每 2 小时更新一次，并读取最近 7 天内每天最后一份成功研判作为趋势分析上下文；输入会按公共主题均衡抽样，并附带主题、标签、重复传播和过去 7 日基线统计。资料不足时会按需调用网页搜索补充公开数据，并在 `research_sources` 返回实际使用的检索来源。研判生成趋势分析、关键数据、后续信号、商业与机遇、就业与技能、风险预警和观察方向；要点带证据强度与 A/B/C 证据等级。有可靠来源时返回一张热点主图、图注和来源链接，优先避开最近 32 小时用过的配图，候选耗尽时使用最久未用的一张。响应中的 `sources` 只包含被正文引用的新闻并保留原始 `index`，`source_indexes` 按它对应。
- 每条包含 `title/summary`（中文）、`title_en/summary_en`、`url`、`source`、`published_at`、`first_seen_at`、`tags/tags_en`、`importance`（1–5）、`pinned`；图片新闻条目另带 `image`（tmpfile 图片地址、图注、来源文章与署名）。历史条目的 `tags_en` 可能为空。
- 每个主题带 `revision`，供首页判断后台刷新是否需要重置该主题的分页。
- 全部开放 CORS，带 `ETag` 与 `Cache-Control: public, max-age=60, stale-while-revalidate=300`，浏览器和 Cloudflare 都能直接复用；错误响应同样带 CORS 且 `no-store`。

```json
{"day":"2026-09-03","timezone":"Asia/Shanghai","generated_at":"…","topics":[{"id":"ai","name":"AI 行业动态","name_en":"AI Industry","description":"…","latest_day":"2026-09-03","updated_at":"…","revision":7,"total":14,"next":null,"items":[{"id":"…","title":"…","summary":"…","title_en":"…","summary_en":"…","url":"https://…","source":"Reuters","published_at":"…","first_seen_at":"…","tags":["OpenAI"],"tags_en":["OpenAI"],"importance":5,"pinned":false,"image":null}]}]}
```

## 工作方式

- **调度**：每个主题有间隔（分钟）、时间窗（整点）、星期和每日上限，全部按 `NEWS_TIMEZONE`（默认 Asia/Shanghai）解释。结束时间早于开始时间表示跨午夜，例如 `22:00–06:00`；跨午夜窗口的星期归属开始运行的那一天。调度器每 30 秒认领到期主题并异步抓取，一次抓取通常 40–130 秒；失败按 15 分钟起指数退避，不超过正常间隔。
- **增量**：每次把最近两天已收录的标题和链接交给模型，只要新增或有实质进展的条目；服务端再按规范化 URL 指纹和标题相似度去重。当天的摘要随时间累积，而不是每小时整页替换。
- **时间显示**：API 同时保留来源发布时间 `published_at` 和 NEWS 收录时间 `first_seen_at`；页面卡片的相对时间以 `first_seen_at` 为准，避免来源只提供日期时被误显示成数小时前。
- **三层数据**：读路径 内存 → Redis → PostgreSQL，逐级回填；写路径 PostgreSQL 事务提交 → Redis → 原子替换内存快照。启动时预热今天与昨天，第一次请求就命中内存。每天 00:00 淘汰前天的内存快照；Redis key 48 小时过期；新闻条目默认保留 128 天（可用 `NEWS_RETENTION_DAYS` 调整）。`REDIS_ADDR` 为空时自动降级为内存 + PostgreSQL。全部主题的首页 `/digest` 按 `limit` 预渲染并连同 ETag 常驻内存，`/days` 与 `/insight` 的响应体同样常驻，写入、日切与清理时一起失效。
- **首页加载**：首屏数据并行请求，只有一轮网络往返；登录状态由登录时写入的可读 cookie `news_role` 判断，匿名访客不会请求任何私有接口。打开根路径时 `index.html` 里的 `<link rel="preload">` 让 `/digest?limit=12`、`/days`、`/topics` 随 HTML 一起出发，不等 `app.js`；带 `topic`/`day` 的深链和已登录账户用不到这三份预加载，会多出三次很小的请求。每个主题首屏 12 条，组尾有一个空的哨兵元素，进入视口即用 `next` 游标静默续拉 12 条；当天所有主题读完才加载前一天，读者快速滑过的主题由底部加载器按顺序补齐，浏览器滚动锚定保证页面不跳动。后台 5 分钟刷新按主题版本决定是否保留已加载内容；版本变化时从新首屏重新开始，隐藏条目随之消失、新置顶的浮到顶部。
- **自动清理**：服务启动时和之后每小时分批清理数据；抓取运行记录保留 3 天，AI 研判历史保留 32 天，新闻条目按 `NEWS_RETENTION_DAYS` 保留，避免依赖外部定时任务。
- **单实例**：内存快照只在本进程内失效，多实例部署会长期返回旧数据。
- **成本**：每次抓取约 $0.06–0.10（`usage.cost_in_usd_ticks`，1 tick = 1e-10 USD）。工作台的管理员运行记录按次显示 tokens、搜索次数和成本，以及今日合计。
- **个人主题**：登录后首页保留公共主题，并追加当前账户启用的个人主题；`/me/` 负责管理个人主题。个人主题不会进入公共 digest；管理员主题仍是公共内容源。
- **公共主题管理**：管理员在同一个 `/me/` 工作台编辑公共主题；打开 `𝐏` 后可直接修改“关注方向”（`brief`），也可调整名称、公开说明、抓取间隔、时间窗、模型、条数和启停状态。保存后会重新排程并刷新公共目录，主题 ID `p` 保持图片新闻行为。
- **个人排程**：个人主题默认每天 06:00–24:00 运行，用户只需设置抓取间隔；Free/Pro 的最短间隔分别为 512/64 分钟。
- **访问统计**：公共首页通过同源 `stats.js` 向 Stats 发送匿名页面访问；查询参数、锚点、主题切换和无限下拉不会重复计数。统计失败不影响页面使用，工作台不接入统计。
- **页面广播**：公共首页复用 Broadcast 的 `feed_news?mode=sequence`；没有有效消息时通知条自动隐藏。
- **图片新闻**：公共主题 `p`（显示为 `𝐏`）紧跟 `𝐗`，按两列响应式图片卡片展示全球热点、X 热帖和美图热图（包括人物写真、美女与时尚街拍）。允许来源明确标注的 AI 生成图；占位图、默认缩略图和过小图片会被丢弃。采集到的 HTTPS 图片由 NEWS 服务端上传到 `TMPFILE_BASE_URL/upload`，页面只加载返回的 `direct_link`；启动时会重新同步最近一周的图片和已有主图，旧图片失效时卡片自动退回文字内容。tmpfile 默认有效期为 36 小时，生产环境建议按需要将其 `FILE_EXPIRATION` 调整到 32 天。

## 本地运行

需要 Rust 1.94+ 与 PostgreSQL；Redis 仅用于生产邮件队列和跨重启缓存，本地可不配。

```bash
cp .env.example .env   # 填写 DATABASE_URL、NEWS_ADMIN_EMAILS、AI_API_KEY、TMPFILE_*
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
| `CAT_IMAGE_FLARE_MODEL` / `CAT_IMAGE_SUNBURST_MODEL` | `gpt-image-2.5-flare` / `gpt-image-2.5-sunburst` | 旅行猫咪日常图片与重要图片模型 |
| `CAT_IMAGE_FALLBACK_MODEL` | `grok-imagine-image-quality` | GPT 图片生成失败时的备用模型 |
| `CAT_IMAGE_GENERATION_TIMEOUT_SECS` | `900` | 单次旅行图片生成总超时 |
| `AI_RESPONSES_TIMEOUT_SECS` | `900` | 新闻领读、旅行叙事等 Responses 请求超时 |
| `NEWS_TIMEZONE` | `Asia/Shanghai` | 天边界与时间窗 |
| `NEWS_SCHEDULER_ENABLED` | `true` | `false` 时只提供页面与 API |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `1800` / `2` | 新闻抓取、综合研判与 AI Agent 共用超时，默认为 30 分钟 |
| `AI_INSIGHT_MODEL` / `AI_INSIGHT_INTERVAL_MINUTES` | `gpt-6-astra` / `120` | 综合研判模型与更新间隔 |
| `NEWS_RETENTION_DAYS` | `128` | 新闻条目保留天数，超过期限自动清理 |
| `TMPFILE_BASE_URL` / `TMPFILE_API_KEY` | 必填 | 服务端图片转存接口与 Bearer 密钥；API key 只存在 NEWS 服务端 |
| `NEWS_IMAGE_MAX_BYTES` / `NEWS_IMAGE_TIMEOUT_SECS` | `8388608` / `300` | 单张新闻图片大小与抓取超时 |
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
GET    /api/me/feed?topic=&day=&limit=&after=
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

GET    /api/cat/state             # 登录后读取领养、身份和成长状态
POST   /api/cat/adoption/preview  # 生成服务端保存的领养身份预览
POST   /api/cat/adoption/confirm  # 确认领养并永久固定身份与初始配饰
POST   /api/cat/reading            # 记录阅读新闻并形成猫咪记忆
GET    /api/cat/trips              # 读取旅程历史
GET    /api/cat/album              # 读取已收藏的旅行明信片
GET    /api/cat/atlas              # 读取成长配饰和世界记忆
GET    /api/cat/encounters         # 读取多猫相遇记录
GET    /api/cat/guide              # 手动获取旧版猫咪领读（自动生活循环不再依赖它）
```

旅行猫咪每 4 小时自动生成一张新的生活照片。每次动态先由叙事模型以猫咪第一人称决定「此刻在做什么」：输入本地时间、在家或旅途中、固定身份与性格、最近十余条动态（避开重复意象）以及图片新闻提供的地点和氛围，输出动态文案、情绪、动作、道具、是否有旅伴同框和给图片模型的画面方向；模型不可用时才退回到一小组固定活动轮换。旅途中可以有一位青春靓丽的美丽少女旅伴同框，合照时按从头到脚完整入镜的全身像取景。图片新闻主题 `p` 只提供地点和视觉线索，不再驱动猫咪阅读全球新闻。每张动态照片都会写入旅程日志、图鉴记忆并避开上一轮的创意方向。图片生成优先调用 GPT Image 2.5，失败后切换 Grok Image；两个提供商都失败时使用 NEWS 原图或模板明信片。所有结果都上传到必填的 `TMPFILE_BASE_URL`，浏览器只读取媒体服务返回的 HTTPS 地址。

管理员状态接口还返回 `users.active_7d` 和 `users.registered`；工作台将它们以
`近 7 天活跃用户/注册用户` 的比例显示在状态行中。注册总数用 migration 执行时的当前用户数初始化，之后由数据库事务内维护的计数器提供；近 7 天活跃数只使用成功登录后写入的最后登录时间，不重建历史登录记录。

个人主题的 `GET /api/me/topics/{id}/items` 在省略 `day` 时返回该主题最近有内容的一天；传入 `day=YYYY-MM-DD` 可查看指定日期。

登录后首页使用 `GET /api/me/feed` 读取公共主题和当前账户启用的个人主题。响应包含 `topics`（主题卡片）、`days`（所选主题有内容的日期，最新优先）以及与公共 `/digest` 同形的 `digest`；`topic`、`day`、`limit`、`after` 与 `next` 的语义与公共接口一致。该响应按会话隔离并始终 `no-store`，不会进入公共 Redis 或内存缓存；个人条目仍只会在当前账户的响应中出现。

登录发送频率限制与 broadcast 相同：同一邮箱 60 秒内不重发，10 分钟内最多 3 次，全站 60 秒内最多 120 次。

登录成功时除 HttpOnly 的会话 cookie 外，还会写入脚本可读的 `news_role`（值为 `user` 或 `admin`，有效期与会话相同）。它只用于首页决定先请求公共内容还是 `/api/me/feed`，服务端对该请求仍按会话鉴权；退出登录时两者一起清除，会话失效时首页也会自行清掉它。

Pro 订单固定为 ¥99 / 31 天，通过 YPay 的 store-token 接口创建微信 Native 订单。YPay 租户使用 webhook fulfillment，回调地址为 `https://xabcnews.com/api/webhooks/payment`，并将租户密钥与 `PAYMENT_WEBHOOK_SECRET` 保持一致；服务端只接受固定 Pro 金额，回调验签后延长 `users.pro_until`。

`/api/plans` 返回套餐列表和 `checkout_enabled`；未配置完整 YPay 三项时，个人页会保留套餐说明但不显示可点击的升级按钮。

## 部署

服务监听容器内 `8788`，`docker-compose.yml` 只绑定宿主机 `127.0.0.1:8793`；把 `xabcnews.com` 的反向代理 upstream 指向它。`.env`（`0600`）提供 `NEWS_ADMIN_EMAILS`、`AI_API_KEY`、SMTP `EMAIL_PASS` 以及启用支付所需的 YPay 三项配置。YPay 侧需先创建与 `YPAY_TENANT_CODE` 对应的 NEWS 租户，并把 webhook fulfillment URL 指向上面的回调地址：

```bash
cd /home/ywgx/1/news
printf 'NEWS_ADMIN_EMAILS=admin@example.com\nAI_API_KEY=…\nEMAIL_PASS=…\nTMPFILE_BASE_URL=https://dl.xabcnews.com\nTMPFILE_API_KEY=…\nYPAY_BASE_URL=…\nYPAY_TENANT_CODE=news\nYPAY_STORE_API_KEY=…\nPAYMENT_WEBHOOK_SECRET=…\n' > .env && chmod 0600 .env
docker-compose config >/dev/null
docker-compose up -d
```

数据库会按顺序自动执行 `migrations/` 中的 migration；当前版本按全新部署设计，请使用空数据库，或重建 `postgres_data` volume 后再启动。

### 反向代理与边缘缓存

`deploy/nginx/xabcnews.com.conf` 是宿主机 nginx 的站点配置：upstream 指向 `127.0.0.1:8793` 并保持长连接，`/api` 与 `/me` 直通不缓存，其余公共路径按服务返回的 `Cache-Control` 做源站微缓存（并发请求合并、过期后台刷新、容器重启或出错时继续提供最近一份）。安装：

```bash
nginx -V 2>&1 | grep -o -- '--conf-path=[^ ]*'                       # 确认主配置位置，下面以 /etc/nginx 为例
sudo mkdir -p /var/cache/nginx /etc/nginx/conf.d && sudo chown ywgx /var/cache/nginx   # 缓存目录归 nginx worker 用户
sudo cp deploy/nginx/xabcnews.com.conf /etc/nginx/conf.d/
grep -n 'conf.d/\*.conf' /etc/nginx/nginx.conf                       # 没有输出就在 http {} 块内加一行 include /etc/nginx/conf.d/*.conf;
sudo nginx -t && sudo nginx -s reload
sudo nginx -T | grep -n 'xabcnews.com\|proxy_cache news_cache'  # 能看到 server_name 和 proxy_cache 行才算生效
curl -sI https://xabcnews.com/digest | grep -i x-cache-status   # 连续两次，第二次应为 HIT
```

主配置里如果已经有一个 `server_name xabcnews.com` 的旧 server 块，需要删掉，否则 nginx 只保留先出现的那一个并告警 conflicting server name。

Cloudflare 默认只缓存静态后缀，`/`、`/digest`、`/topics`、`/days`、`/insight` 每次都回源。要让边缘直接命中，在 Cloudflare 控制台为该域添加一条 Cache Rule，表达式：

```text
(http.host eq "xabcnews.com" and not starts_with(http.request.uri.path, "/api") and not starts_with(http.request.uri.path, "/me"))
```

缓存资格选「Eligible for cache」，Edge TTL 选「Use cache-control header if present, bypass cache if not」，Browser TTL 选「Respect origin」。服务对错误响应和所有私有接口都返回 `no-store`，不会被误缓存；公共响应对所有访客相同，不依赖 cookie。

CI 与 broadcast 相同：`quality-gate.yml` 执行 `make release-check`；`build-deploy.yml` 在 `main` push 后构建并推送 `rust`、`<date>-rust`、`<version>-rust` 标签，commit message 含 `prod` 或手动触发时部署生产；`manual-deploy-prod.yml` 直接部署当前镜像。

## 验证

```bash
make release-check   # fmt、test、clippy、全部 JS 语法、compose 配置
```

单元测试覆盖工作台角色边界、调度计算、URL 规范化与指纹、标题相似度、条目校验、真实 Responses 返回样例（`tests/fixtures/`）的解析、ETag/304。
