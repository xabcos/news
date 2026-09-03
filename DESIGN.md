# NEWS 设计方案

> 项目：`/root/xabcos/news`，域名 `news.xairouter.com`，镜像 `proxyxai/news:rust`，宿主机端口 `127.0.0.1:8793`。
> 本文件是设计的唯一权威记录。会话中断后先读本文件，再看文末「进度与待确认」继续。
> 最后更新：2026-09-03（实现完成，见 §15 与 §16）。

## 0. 一句话

一个独立的 Rust/Axum 服务：管理员在页面上定义若干「内容方向」（topic）和抓取节奏，服务按节奏调用 grok-4.6（Responses API，web_search + x_search 固定开启）抓取最新新闻，按天写入 PostgreSQL，热数据放 Redis 和进程内存，对外提供无鉴权的公共 JSON API 和一个简洁的新闻聚合页；xai-docs 顶部菜单增加「新闻」页，直接消费该 API。

## 1. 已验证的前提（2026-09-03 实测）

| 事实 | 结论 |
| --- | --- |
| chat completions + `search_parameters` | xAI 已废弃，返回 "Live search is deprecated" |
| `POST /v1/responses` + `tools:[{"type":"web_search"},{"type":"x_search"}]` | 可用，返回真实 URL、来源、发布时间 |
| `text.format` 严格 `json_schema` 与搜索工具同时使用 | 可用，输出可直接 `serde_json` 解析 |
| `reasoning.effort = "low"` | 被接受；耗时 113s → 42s，质量不降 |
| `max_tool_calls` | 不被遵守（要 4 给 8，要 6 给 13），不能依赖 |
| `usage.cost_in_usd_ticks` | 与官方定价吻合的换算是 1 tick = 1e-10 USD；单次抓取约 $0.06–0.10，40–130 秒 |
| sonar / gpt-5.5 经 zairouter | 不能联网；gpt-5.5 会被注入 Codex 系统提示。不可用 |
| 模型列表 | 仅 `grok-4.6` 具备搜索能力；默认模型固定为它，保留 env/主题级覆盖 |

复现命令（key 从环境变量读，不写入任何文件）：

```bash
curl -sS https://api.zairouter.com/v1/responses \
  -H "Authorization: Bearer $AI_API_KEY" -H "Content-Type: application/json" \
  -d '{"model":"grok-4.6","reasoning":{"effort":"low"},
       "instructions":"你是新闻编辑。只输出 JSON。",
       "input":"检索过去 24 小时内 AI 领域最重要的 5 条新闻。",
       "tools":[{"type":"web_search"},{"type":"x_search"}],
       "text":{"format":{"type":"json_schema","name":"news_digest","strict":true,"schema":{...}}}}'
```

成功样例保存在 `/tmp/grok_structured.json`、`/tmp/grok_low4.json`、`/tmp/grok_x6.json`（临时文件，可能已不存在；实现阶段应把一份脱敏样例放进 `tests/fixtures/`）。

## 2. 总体架构

沿用 `/root/xabcos/broadcast` 的骨架，能复用的直接复用（auth.rs、email.rs、error.rs、config/state 的连接管理、web.rs 的安全头与 Origin 校验、Dockerfile、Makefile、三条 GitHub Actions、docker-compose 结构）。

```
                 ┌──────────────── news (单进程, Axum) ────────────────┐
 浏览器/文档站 ──▶ 公共路由 /digest /topics /days /feed.xml  ◀─ 内存快照 │
 管理员 ────────▶ /admin/ 页面 + /api/*（Cookie 会话）                   │
                 │  调度器(每 30s tick) ──▶ 抓取任务 ──▶ grok-4.6         │
                 │        │                    │  (Responses API)         │
                 │        ▼                    ▼                          │
                 │   topics.next_run_at   校验/去重/落库 ──▶ PG 事务提交    │
                 │                              └─▶ Redis 写入 ─▶ 内存替换 │
                 └──────────────────────────────────────────────────────┘
      读路径：内存 → Redis → PG（miss 逐级回填）；启动时预热，正常情况下请求 100% 命中内存。
```

设计原则：

- 单实例部署（与 broadcast 相同的前提）。内存快照是权威热数据；Redis 让重启后秒级预热并可被其他进程读取；PG 是按天归档的事实来源。
- Redis 可选。`REDIS_ADDR` 为空时自动降级为内存 + PG，本地开发无需 Redis（登录走开发链接）。
- 抓取是增量的：每次只补充当天尚未收录的条目，当天摘要随时间累积。
- 一切模型输出都当数据，不当 HTML；URL 只接受 http/https。
- API key 只存在于环境变量，不进数据库、页面、日志、错误信息。

## 3. 数据模型（PostgreSQL，`migrations/0001_initial.sql`）

认证四表 `users`、`email_login_tokens`、`sessions` 与 broadcast 完全一致（照搬）。

### 3.1 topics（内容方向）

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| id | TEXT PK | slug，`^[a-z0-9][a-z0-9-]{0,31}$`，管理员创建时指定，不可改，用于 `?topic=ai` |
| name | TEXT | 中文显示名，1–60 字 |
| name_en | TEXT | 英文显示名，可空则回退 name |
| description | TEXT | 公开的一句话说明，≤200 字，可空 |
| brief | TEXT | 给模型的方向描述，1–2000 字，不公开 |
| enabled | BOOL | 默认 true |
| interval_minutes | INT | 15–1440，默认 60 |
| window_start | INT | 整点小时 0–23，默认 6 |
| window_end | INT | 整点小时 1–24，默认 24，须 > window_start |
| weekdays | SMALLINT[] | ISO 星期数组，1=周一；默认 {1,2,3,4,5}。数组直接映射 `Vec<i16>`，API 与库表零转换 |
| model | TEXT | 默认 `grok-4.6` |
| reasoning_effort | TEXT | `low|medium|high`，默认 low |
| max_items | INT | 每次最多收录条数 1–30，默认 8 |
| lookback_hours | INT | 只收录该时间窗内发布的报道 1–72，默认 24 |
| daily_run_cap | INT | 每日最多运行次数 1–96，默认 24（成本保险） |
| sort_weight | INT | 页面上的排序，默认 0 |
| next_run_at | TIMESTAMPTZ | 下次计划运行；NULL 表示未排程 |
| last_run_at / last_status | TIMESTAMPTZ / TEXT | 最近一次运行 |
| consecutive_failures | INT | 连续失败次数，用于退避 |
| created_at / updated_at | TIMESTAMPTZ | |

时区不按主题设置：全局 `NEWS_TIMEZONE`（默认 Asia/Shanghai）同时决定「天」的边界和时间窗。

### 3.2 runs（每次抓取）

| 字段 | 说明 |
| --- | --- |
| id UUID PK, topic_id FK ON DELETE CASCADE | |
| triggered_by | `schedule` / `manual` |
| status | `running` / `ok` / `failed` / `interrupted` |
| started_at, finished_at | |
| model, response_id | 模型与 xAI 返回的 response id |
| input_tokens, cached_tokens, output_tokens, reasoning_tokens | 来自 usage |
| web_search_calls, x_search_calls | 来自 usage.server_side_tool_usage_details |
| cost_ticks BIGINT | usage.cost_in_usd_ticks 原值；展示时 ÷ 1e10 |
| returned_count, new_count, dup_count, dropped_count | 模型返回数 / 新增 / 命中已存在 / 校验丢弃 |
| error TEXT | 失败原因（不含密钥） |
| payload JSONB | 模型最终 message 文本解析后的 items（脱敏，≤256 KB），便于排查 |

索引：`(topic_id, started_at DESC)`。

### 3.3 items（新闻条目，按天）

| 字段 | 说明 |
| --- | --- |
| id UUID PK | |
| topic_id FK ON DELETE CASCADE | |
| day DATE | 首次抓到时的本地日期（NEWS_TIMEZONE），不是发布日期 |
| fingerprint BYTEA(32) | 见 5.4 |
| title, summary | 中文标题（≤200 字）、中文摘要（≤600 字） |
| title_en, summary_en | 英文标题、英文摘要（待确认是否保留，见 §11） |
| url TEXT | 规范化前的原始 URL，≤2000 |
| source TEXT | 来源名，≤80 |
| published_at TIMESTAMPTZ NULL | 模型给出的发布时间，UTC |
| tags TEXT[] | ≤6 个，每个 ≤30 字 |
| importance SMALLINT | 1–5 |
| first_seen_at, last_seen_at, seen_count | 首次/最近出现于哪次抓取，出现次数 |
| first_run_id UUID | 首次收录的 run |
| hidden BOOL, pinned BOOL | 管理员隐藏 / 置顶 |
| created_at, updated_at | |

约束与索引：

- `UNIQUE (topic_id, day, fingerprint)`
- `INDEX (topic_id, day, hidden, published_at DESC, first_seen_at DESC)`
- `INDEX (topic_id, fingerprint, day DESC)`（跨天去重查上一天）

## 4. 调度器

- `tokio::interval(30s)`，`MissedTickBehavior::Skip`。
- 每个 tick：`SELECT id FROM topics WHERE enabled AND next_run_at <= NOW() ORDER BY next_run_at LIMIT k`，k = `NEWS_MAX_CONCURRENT_RUNS`（默认 2）减当前在飞任务数；对每个候选在同一事务里 `INSERT INTO runs (status='running')` 并把 `topics.next_run_at = NULL`，用 `SELECT … FOR UPDATE SKIP LOCKED` 避免重复认领。认领成功即 `tokio::spawn` 抓取任务，任务整体套 `NEWS_FETCH_TIMEOUT_SECS`（默认 300s）超时。
- 同一主题同时只允许一个在飞任务（内存 `HashSet<topic_id>` 加 runs 表 running 状态双重保证）。
- 启动时：把所有 `status='running'` 的 runs 标为 `interrupted`；为所有 `enabled AND next_run_at IS NULL` 的主题计算 `next_run_at`。
- 手动触发 `POST /api/topics/{id}/run`：若该主题在飞则 409；否则立即运行，忽略时间窗与每日上限，`trigger='manual'`。

`next_run_at` 计算（全部在 NEWS_TIMEZONE 下）：

```
fn next_run(after: DateTime<Tz>, topic, today_runs, failures) -> DateTime<Tz>
  candidate = after + interval_minutes
  if failures > 0:  candidate = after + min(interval, 15min * 2^(failures-1))
  if today_runs >= daily_run_cap:  candidate = 次日 00:00
  loop:
    d = candidate.date()
    if weekday(d) ∉ weekdays: candidate = (d+1) 的 window_start; continue
    if candidate.time < window_start: candidate = d 的 window_start; break
    if candidate.time >= window_end:  candidate = (d+1) 的 window_start; continue
    break
  return candidate
```

任何修改了 enabled/interval/window/weekdays 的管理写操作都会重算 `next_run_at`（禁用则置 NULL）。

## 5. 抓取流水线（fetch.rs）

### 5.1 请求

```jsonc
POST {AI_BASE_URL}/v1/responses
{
  "model": topic.model,
  "reasoning": { "effort": topic.reasoning_effort },
  "store": false,
  "tools": [ { "type": "web_search" }, { "type": "x_search" } ],
  "instructions": <5.2 系统提示>,
  "input": <5.2 用户输入>,
  "text": { "format": { "type": "json_schema", "name": "news_digest", "strict": true, "schema": <5.3> } }
}
```

reqwest：连接超时 10s，总超时 = NEWS_FETCH_TIMEOUT_SECS；对 429 / 5xx / 网络错误重试 2 次（10s、30s），其他 4xx 直接失败。`status != "completed"` 或 `error != null` 视为失败。

### 5.2 提示词

系统提示（instructions）：

```
你是一名严谨的新闻编辑，为「{name}」栏目整理最新资讯。
规则：
1. 只收录发布时间在最近 {lookback_hours} 小时内、可以核实的真实报道；每条必须给出指向具体文章页面的 URL，不能是站点首页或栏目页。
2. 先用 web_search 和 x_search 检索，再综合判断重要性；同一事件只保留一条，优先一手来源或权威媒体。检索次数控制在 {search_budget} 次以内。
3. 「已收录」列表中的事件不要重复，除非出现实质性新进展。
4. 每条给出中文标题、中文摘要（1 到 2 句，说清发生了什么、为什么重要），以及英文标题、英文摘要。
5. importance 取 1 到 5，5 表示行业级重大事件；tags 给 1 到 5 个短标签。
6. 最多返回 {max_items} 条；没有符合条件的新内容时返回空数组。
7. 严格按 JSON Schema 输出，不输出任何其他文字。
```

用户输入（input）：

```
栏目方向：{brief}
当前时间：{now_local}（{tz}），UTC {now_utc}
已收录（{n} 条，取今天与昨天，最多 80 条）：
- {title} | {url}
...
```

### 5.3 输出 Schema（strict）

```json
{"type":"object","additionalProperties":false,"required":["items"],
 "properties":{"items":{"type":"array","items":{"type":"object","additionalProperties":false,
   "required":["title","summary","title_en","summary_en","url","source","published_at","tags","importance"],
   "properties":{
     "title":{"type":"string"},"summary":{"type":"string"},
     "title_en":{"type":"string"},"summary_en":{"type":"string"},
     "url":{"type":"string"},"source":{"type":"string"},
     "published_at":{"type":["string","null"]},
     "tags":{"type":"array","items":{"type":"string"}},
     "importance":{"type":"integer"}}}}}}
```

解析：取 `output[]` 中 `type == "message"` 的 `content[]` 里 `type == "output_text"` 的 `text`，`serde_json` 反序列化；失败则尝试截取首个 `{` 到最后一个 `}`；仍失败记 `failed: invalid json`。

### 5.4 校验、规范化、去重

逐条校验，不合格的丢弃并计入 `dropped_count`：

- title/summary 非空且不超长；title_en/summary_en 超长截断。
- url 可解析、scheme ∈ {http, https}、有 host、无用户名密码。
- published_at 解析 RFC3339 或 `YYYY-MM-DD`；早于 `now − lookback − 12h` 的丢弃（防止模型混入旧闻）。
- tags 去重、截断到 6 个；importance clamp 到 1–5。

规范化 URL → 指纹：小写 scheme/host，去默认端口，去 `www.` 前缀，去 fragment，删除跟踪参数（`utm_*`、`fbclid`、`gclid`、`igshid`、`ref`、`ref_src`、`spm`、`share_*`），去尾部 `/`，`fingerprint = sha256(canonical_url)`。

去重顺序：

1. 同主题、`day ∈ {今天, 昨天}` 中已有相同 fingerprint → 视为重复：更新该条 `last_seen_at/seen_count`，`dup_count += 1`，不新增。
2. 否则与今天/昨天内存快照中的条目做标题比对：`normalize(title) = 小写、去标点空白、全角转半角`，完全相同或一方为另一方前缀且长度 ≥ 12 → 重复，同上处理。
3. 否则 `INSERT`，`day = 本地今天`，`first_seen_at = NOW()`。

### 5.5 落库与刷新

一个事务内：写 items、更新重复条目、写 runs（status/usage/计数）、更新 topics（last_run_at、last_status、consecutive_failures、next_run_at）。提交后重建该主题「今天」的快照（§6），先写 Redis 再原子替换内存。失败的运行不触碰快照。

## 6. 缓存层（digest.rs）

### 6.1 结构

```rust
struct Snapshot {              // 一个主题一天，也是 Redis 里的值
    updated_at: Option<DateTime<Utc>>,   // 最近一条新增条目的 first_seen_at
    items: Vec<PublicItem>,    // 可见条目，已排序（pinned 优先，其后 published_at desc, first_seen_at desc）
}
// 主题元信息（id, name, name_en, description）单独缓存在 topics 列表里，管理端写入后整体重载，
// 因此 Redis 中的快照不会因为改名而陈旧。
struct DigestCache {
    days:   RwLock<HashMap<(TopicId, NaiveDate), Arc<Snapshot>>>,      // 只放今天+昨天；归档天走 Redis → PG，不占内存
    hot:    RwLock<Option<Arc<Rendered>>>,   // 默认请求 (/digest 无参数) 的预渲染 body + ETag
    topics: RwLock<Arc<Vec<TopicMeta>>>,     // 启用中的主题列表
    generation: AtomicU64,                   // 与 broadcast FeedCache 相同的防陈旧写入机制
}
```

- 读路径：`snapshot(topic, day)` → 内存命中直接返回；否则 Redis `GET news:v1:{topic}:{day}` → 命中则反序列化；否则 PG 查询 → 写 Redis（TTL 48h，归档天 6h）。只有今天和昨天会放入内存，归档天不占内存。
- 写路径（抓取成功、管理员隐藏/置顶/删除）：PG 提交 → 重建快照 → Redis `SET` → 内存替换 → `hot` 置空（下次请求重建）。
- 启动预热：加载 topics；对每个启用主题的今天与昨天执行一次 `get`。启动完成后第一次请求即命中内存。
- Redis 写失败只记 warn，不影响请求（内存与 PG 仍一致）。

### 6.2 换日与保留

- 维护任务每分钟检查本地日期；日期变化时淘汰 `day < 昨天` 的内存快照并清空 `hot`。今天的快照在第一条新闻写入时创建。
- Redis 靠 TTL 自然过期。PG 永久保留；`NEWS_RETENTION_DAYS > 0` 时按批删除更早的 items。
- `latest_day` 解析：聚合请求取所有启用主题中有可见条目的最近一天（先看内存今天/昨天，都没有再查 PG `MAX(day)` 并缓存到下次写入）；单主题请求按该主题解析。响应始终只包含一个 `day`，各主题另带自己的 `latest_day`，避免一页混合多天。

### 6.3 响应缓存

与 broadcast 一致：`ETag` 为 body 的 SHA-256 前 128 位，支持 `If-None-Match`（弱标签、`*`），`Cache-Control: public, max-age=60, stale-while-revalidate=300`，`Access-Control-Allow-Origin: *`。错误响应 `no-store` 且同样带 CORS。公共 JSON 走 tower-http gzip 压缩（已有 `Vary: Accept-Encoding`）。

## 7. 公共 API（根路径，无鉴权）

```
GET /digest?topic=ai&day=2026-09-03&limit=30
GET /topics
GET /days?topic=ai&limit=30
GET /feed.xml?topic=ai
```

参数：`topic` 为 slug（可省略 = 全部启用主题）；`day` 为 `YYYY-MM-DD`（可省略 = latest_day）；`limit` 1–100，默认 30，按主题各自截断。非法参数返回 400 JSON。

`/digest` 响应：

```json
{
  "day": "2026-09-03",
  "timezone": "Asia/Shanghai",
  "generated_at": "2026-09-03T08:05:12Z",
  "topics": [
    {
      "id": "ai",
      "name": "AI 行业动态",
      "name_en": "AI Industry",
      "description": "大模型、芯片、融资与政策",
      "latest_day": "2026-09-03",
      "updated_at": "2026-09-03T08:03:40Z",
      "total": 14,
      "items": [
        {
          "id": "6f1c…",
          "title": "美国司法部在纽约时报诉 OpenAI 案中支持 OpenAI",
          "summary": "司法部首次介入……",
          "title_en": "Justice Dept. Sides With OpenAI in NYT Copyright Suit",
          "summary_en": "…",
          "url": "https://www.nytimes.com/2026/09/02/technology/…",
          "source": "The New York Times",
          "published_at": "2026-09-02T18:41:00Z",
          "first_seen_at": "2026-09-03T00:03:10Z",
          "tags": ["OpenAI", "版权"],
          "importance": 5,
          "pinned": false
        }
      ]
    }
  ]
}
```

`/topics`：`{"topics":[{"id","name","name_en","description","latest_day","today_count","updated_at"}]}`。
`/days`：`{"topic":"ai"|null,"days":[{"day":"2026-09-03","count":14},…]}`，最近的天在前。
`/feed.xml`：Atom 1.0，最近 50 条可见条目（`first_seen_at` 倒序），entry.id 为 `urn:uuid:{id}`，link 指向原文，author 为来源，category 为 tags。

浏览器侧只用简单 GET 与 `Accept` 头，不触发预检；与 xai-docs 现有 `wall.js` 的调用方式一致。

## 8. 管理 API（`/api/*`，同源 Cookie 会话，照搬 broadcast 的 Origin/Sec-Fetch-Site 校验）

```
POST   /api/auth/email/start | consume | logout      GET /api/me
GET    /api/status                                     # 在飞任务、各主题 next_run_at、今日成本合计
GET    /api/topics            POST /api/topics
PATCH  /api/topics/{id}       DELETE /api/topics/{id}
POST   /api/topics/{id}/run                            # 立即抓取，返回 run id；在飞则 409
GET    /api/topics/{id}/runs?limit=50
GET    /api/topics/{id}/items?day=YYYY-MM-DD           # 含隐藏条目
PATCH  /api/items/{id}                                 # {"hidden":bool,"pinned":bool}
DELETE /api/items/{id}
```

调度字段在 API 中与库表同形：`{"interval_minutes":60,"window_start":6,"window_end":24,"weekdays":[1,2,3,4,5]}`（整点小时；ISO 1=周一…7=周日）。

登录链接指向 `PUBLIC_BASE_URL/admin/#login_token=…`；Cookie 名 `news_session`，Path=/。

## 9. 页面（`pages/`，原生 HTML/CSS/JS，无构建步骤，风格延续 broadcast 的视觉体系）

### 9.1 公开聚合页 `/`（index.html, app.js, styles.css, icon.svg）

- 顶栏：任何宽度下都只有一行薄条（约 42px）：品牌「NEWS」、可横向滑动的主题标签（来自 `/topics`）、中/EN 切换（渲染 `*_en` 字段）；下滑时整条滑出屏幕，上滑即回，让列表占满屏幕。深浅色跟随系统。页脚文案「由 XAI 提供支持」链接到 xairouter.com。
- 日期条：一行（约 30px）：`‹ 今天 · 9月3日星期四 · 更新于 HH:MM · N 条 ›`，前后切换的数据来自 `/days`。
- 列表：卡片 = 标题（新窗口链接，`rel=noopener noreferrer`）、来源 · 相对时间（如「2 小时前」）· 重要度徽标（importance ≥ 4 显示「重要」）、摘要、标签；置顶条目带标记；`first_seen_at` 晚于用户上次访问时间（localStorage）的条目显示「NEW」点。
- 每 5 分钟静默 `fetch('/digest…')`（浏览器按 ETag 复用 304），body 变化才重绘。
- 空状态：「今天还没有新内容」并提示最近有内容的日期。
- 所有文本经 `textContent` 渲染；CSP 与 broadcast 相同（`default-src 'self'`）。

### 9.2 管理页 `/admin/`（admin/index.html, admin/app.js 复用 styles.css）

- 登录视图与 broadcast 一致（邮箱一次性链接）。
- 左栏：主题列表，状态点（启用/在飞/失败）、下次运行时间、「新建」。
- 右栏：主题表单（slug、name、name_en、description、brief、enabled、间隔、时间窗、星期、model、effort、max_items、lookback、daily_run_cap）；按钮「保存」「立即抓取」「复制公共地址」「删除」。
- 运行历史表：时间、状态、返回/新增/重复/丢弃、tokens、搜索次数、成本（USD）、耗时、错误。
- 今日条目列表：隐藏 / 置顶 / 删除，改动即时反映到公共快照。
- 顶部状态条：在飞任务数、今日成本合计。
- `/admin` 无尾斜杠时 ServeDir 重定向到 `/admin/`。

## 10. 配置、部署、CI

环境变量（`Config::from_env`，与 broadcast 同样严格校验）：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| BIND_ADDR | 0.0.0.0:8788 | |
| DATABASE_URL / DATABASE_MAX_CONNECTIONS | – / 20 | |
| MIGRATIONS_DIR / PAGES_DIR | migrations / pages | |
| PUBLIC_BASE_URL | – | `https://news.xairouter.com`；非回环必须 https |
| NEWS_ALLOWED_EMAILS | – | 管理员白名单，逗号分隔（与 broadcast 相同两个邮箱） |
| SESSION_COOKIE_NAME / SESSION_TTL_DAYS / EMAIL_LOGIN_LINK_TTL_MINUTES / EMAIL_LOGIN_DEV_LINKS | news_session / 30 / 30 / false | |
| REDIS_ADDR / REDIS_PASS / REDIS_DB | 空 / 空 / 1 | 邮件队列 + 缓存；为空则无 Redis 模式（仅开发） |
| EMAIL_STREAM_PRIORITY / EMAIL_OID / EMAIL_REPLY_TO | important / 0 / 空 | |
| AI_BASE_URL | https://api.zairouter.com | |
| AI_API_KEY | – | 必填（NEWS_SCHEDULER_ENABLED=false 时可空） |
| AI_DEFAULT_MODEL | grok-4.6 | 新建主题的默认值 |
| NEWS_TIMEZONE | Asia/Shanghai | 天边界与时间窗 |
| NEWS_FETCH_TIMEOUT_SECS | 300 | |
| NEWS_MAX_CONCURRENT_RUNS | 2 | |
| NEWS_SCHEDULER_ENABLED | true | false 时只读服务，不花钱 |
| NEWS_RETENTION_DAYS | 0 | 0 = 永久保留 |

Cargo 依赖 = broadcast 的依赖 + `reqwest`（rustls-tls, json, gzip）+ `chrono-tz`；tower-http 增加 `compression-gzip`。

docker-compose.yml：`news`（`127.0.0.1:8793:8788`）、`news-mail`（proxyxai/mail）、`news-redis`、`news-postgres`（postgres:18），结构照搬 broadcast；`AI_API_KEY` 与 `EMAIL_PASS` 从 `.env`（0600）读入而不写在 compose 里。`docker-compose.local.yml` 覆盖为本地构建、开发登录链接、无 Redis。

CI：`quality-gate.yml`、`build-deploy.yml`、`manual-deploy-prod.yml` 与 broadcast 相同；镜像标签 `rust`、`<date>-rust`、`<version>-rust`；生产目录 `/home/ywgx/1/news`。反向代理 `news.xairouter.com → 127.0.0.1:8793`。

## 11. xai-docs 集成（`/root/ywgx/yai/xai-docs`）

按 stats / wall 页面的既有模式：

1. `config.toml` `[extra.services]` 增加 `news_base_url = "https://news.xairouter.com"`。
2. `content/_index.md` 与 `content/_index.en.md` 的 `[[extra.menu.main]]` 增加 `新闻` / `𝑵𝒆𝒘𝒔`，section `news`，url `/news/` 与 `/en/news/`，weight 45（模型 40 与广告 50 之间）。
3. 新增 `content/news/_index.md`、`_index.en.md`（`template = "news.html"`）。
4. 新增 `templates/news.html`：`header(current_section="news")`，容器带 `data-api`、`data-lang`，主题标签、日期条、列表、加载/失败/空状态。
5. 新增 `static/js/news.js`：拉 `/topics` 与 `/digest`，`textContent` 渲染，`lang=en` 时用 `*_en`；`safeUrl` 只放行 https。
6. 新增 `sass/pages/_news.scss` 并在 `main.scss` 引入，使用 `_tokens.scss` 变量。

页面在文档站内渲染，与站内其他页风格一致；不需要 embed 脚本。

## 12. 成本与限制

| 项目 | 数值 |
| --- | --- |
| 单次抓取（effort=low，web + x） | 约 $0.06–0.10，40–130 秒 |
| 工作日 06:00–24:00 每小时 | 18 次/天，约 90 次/周 |
| 单主题月成本 | 约 $25–35 |

成本靠三层约束：提示词中的检索次数预算、`daily_run_cap`、管理页可见的每次成本与今日合计。`max_tool_calls` 不可靠，不使用。

## 13. 测试

- 单元：`next_run` 的窗口/星期/间隔/退避/上限；URL 规范化与指纹；标题归一化；条目校验；Responses 输出解析（fixture）；ETag/304；latest_day 解析；换日淘汰。
- 集成（需 PG，本机 `yai-test-pg` 端口 15432 可建 `news_test` 库）：迁移、去重 upsert、快照重建、公共端点、管理端点权限。
- 手工：本地 `cargo run` + 一次「立即抓取」打真实 API（约 $0.1），检查页面与 API。
- `make release-check`：fmt、test、clippy、js-check、compose-check。

## 14. 实施顺序（可中断续做的检查表）

- [ ] 1 脚手架：Cargo.toml、Dockerfile、Makefile、compose ×2、CI ×3、README、.env.example、migrations/0001_initial.sql；从 broadcast 移植 config/state/error/web/auth/email。
- [ ] 2 主题 CRUD + 调度计算 + 单元测试。
- [ ] 3 抓取：AI 客户端、解析、校验、规范化、去重、落库、运行记账；fixture 测试。
- [ ] 4 调度器循环、手动触发、维护任务（换日、保留、认证清理）。
- [ ] 5 缓存三层 + 公共 API（/digest /topics /days /feed.xml）+ ETag/CORS/gzip。
- [ ] 6 公开聚合页。
- [ ] 7 管理页。
- [ ] 8 本地端到端（真实 API 一次），release-check 通过。
- [ ] 9 xai-docs 集成。
- [ ] 10 部署说明：反代、.env、首次 compose 启动。

## 15. 进度与待确认

已确定：

- x_search 与 web_search 固定同时开启，不设选项。
- 公共 API 直接挂根路径：`/digest`、`/topics`、`/days`、`/feed.xml`，不带 `/api/public`。
- 端口 8793、镜像 `proxyxai/news:rust`、管理员邮箱与 broadcast 相同、默认调度工作日 06:00–24:00 每 60 分钟、effort=low。

用户已于 2026-09-03 确认全部推荐值：双语字段保留；预置主题 `ai`、`openai`、`claude`（后两个围绕官方 X 账号与最新模型，每次最多 5 条）；xai-docs 站内 `/news/` 页面；清理了 codex-cloud/claude-cloud 的 target；保留 `/feed.xml`。代码风格要求：优雅、简洁、高效，避免过度防御的冗余代码。

当前状态：§14 的 1–9 全部完成（2026-09-03），`make release-check` 通过，本地端到端两次真实抓取验证通过。剩余第 10 项部署由用户在生产机执行。

## 16. 实现记录（2026-09-03）

- 代码：`src/` 共 10 个模块，约 2600 行；页面 `pages/`（公开页 + `admin/`）；24 个单元测试。
- 与设计的差异：时间窗改为整点小时；`weekdays` 用 `SMALLINT[]` 而非位掩码；时区改为全局 `NEWS_TIMEZONE`；归档天不进内存；`runs.trigger` 列名为 `triggered_by`；请求缓存的 memo 只针对默认 `/digest`。
- 本地验证（PG 16 on 15432，无 Redis）：公共路由、404/400 JSON 错误、ETag/304（含弱标签）、gzip、`/admin` 307、开发链接登录、主题 CRUD 的约束错误文案、跨源写入 403、并发运行 409；隐藏/置顶立即反映到公共 ETag。
- 两次真实抓取：第一次 57 秒、5 条新增、14+1 次搜索、$0.104；第二次（带"已收录"列表）6 条新增、0 重复、20+1 次搜索、$0.218。模型不总遵守提示词中的检索次数上限，成本主要由搜索次数决定，`daily_run_cap` 是硬保险。
- 重启后 `next_run_at` = 上次开始时间 + 60 分钟；预热后首个 `/digest` 请求 2 ms。
- xai-docs：`config.toml` 增加 `news_base_url`；菜单「新闻」/「𝑵𝒆𝒘𝒔」weight 45；`content/news/`、`templates/news.html`、`static/js/news.js`、`sass/pages/_news.scss`、`static/_headers`；`zola build` 通过。改动尚未提交。
- 未做：Docker 镜像未在本机构建（CI 负责）；news 目录未初始化 git。
