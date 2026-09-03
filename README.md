# NEWS

一个独立的新闻聚合服务。管理员在页面上定义若干「内容方向」和抓取节奏，服务按节奏让 `grok-4.6`（Responses API，`web_search` + `x_search`）检索最新报道，按天写入 PostgreSQL，热数据放 Redis 和进程内存，对外提供无鉴权的公共 JSON API、Atom 源和一个简洁的聚合页。

生产地址：`https://news.xairouter.com`，管理页 `/admin/`。设计说明见 [`DESIGN.md`](DESIGN.md)。

## 公共 API

```text
GET https://news.xairouter.com/digest?topic=ai&day=2026-09-03&limit=30
GET https://news.xairouter.com/topics
GET https://news.xairouter.com/days?topic=ai&limit=30
GET https://news.xairouter.com/feed.xml?topic=ai
```

- `topic` 省略时返回全部启用主题；`day` 省略时返回最近一个有内容的日期（响应里的 `day` 字段说明是哪一天）；`limit` 1–100，默认 30，按主题各自截断。
- 每条包含 `title/summary`（中文）、`title_en/summary_en`、`url`、`source`、`published_at`、`first_seen_at`、`tags`、`importance`（1–5）、`pinned`。
- 全部开放 CORS，带 `ETag` 与 `Cache-Control: public, max-age=60, stale-while-revalidate=300`，浏览器和 Cloudflare 都能直接复用；错误响应同样带 CORS 且 `no-store`。

```json
{"day":"2026-09-03","timezone":"Asia/Shanghai","generated_at":"…","topics":[{"id":"ai","name":"AI 行业动态","name_en":"AI Industry","description":"…","latest_day":"2026-09-03","updated_at":"…","total":14,"items":[{"id":"…","title":"…","summary":"…","title_en":"…","summary_en":"…","url":"https://…","source":"Reuters","published_at":"…","first_seen_at":"…","tags":["OpenAI"],"importance":5,"pinned":false}]}]}
```

## 工作方式

- **调度**：每个主题有间隔（分钟）、时间窗（整点）、星期和每日上限，全部按 `NEWS_TIMEZONE`（默认 Asia/Shanghai）解释。调度器每 30 秒认领到期主题并异步抓取，一次抓取通常 40–130 秒；失败按 15 分钟起指数退避，不超过正常间隔。
- **增量**：每次把最近两天已收录的标题和链接交给模型，只要新增或有实质进展的条目；服务端再按规范化 URL 指纹和标题相似度去重。当天的摘要随时间累积，而不是每小时整页替换。
- **三层数据**：读路径 内存 → Redis → PostgreSQL，逐级回填；写路径 PostgreSQL 事务提交 → Redis → 原子替换内存快照。启动时预热今天与昨天，第一次请求就命中内存。每天 00:00 淘汰前天的内存快照；Redis key 48 小时过期；PostgreSQL 永久保留（可用 `NEWS_RETENTION_DAYS` 限制）。`REDIS_ADDR` 为空时自动降级为内存 + PostgreSQL。
- **单实例**：内存快照只在本进程内失效，多实例部署会长期返回旧数据。
- **成本**：每次抓取约 $0.06–0.10（`usage.cost_in_usd_ticks`，1 tick = 1e-10 USD）。管理页按次显示 tokens、搜索次数和成本，以及今日合计。

## 本地运行

需要 Rust 1.94+ 与 PostgreSQL；Redis 仅用于生产邮件队列和跨重启缓存，本地可不配。

```bash
cp .env.example .env   # 填写 DATABASE_URL、AI_API_KEY
cargo run
```

打开 `http://127.0.0.1:8788/`（公开页）和 `http://127.0.0.1:8788/admin/`（管理页）。`EMAIL_LOGIN_DEV_LINKS=true` 时登录响应直接给出一次性链接，只允许回环地址。`NEWS_SCHEDULER_ENABLED=false` 可以只读运行、不花钱。

完全容器化：`make docker-up-local`。

## 配置

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `DATABASE_URL` | – | PostgreSQL DSN，启动时自动执行 migrations |
| `PUBLIC_BASE_URL` | – | 对外根地址，用于登录链接与 Origin 校验；非回环必须 HTTPS |
| `NEWS_ALLOWED_EMAILS` | – | 管理员邮箱白名单，逗号分隔 |
| `AI_API_KEY` | – | 调用模型的密钥，只存在于环境变量 |
| `AI_BASE_URL` / `AI_DEFAULT_MODEL` | `https://api.zairouter.com` / `grok-4.6` | |
| `NEWS_TIMEZONE` | `Asia/Shanghai` | 天边界与时间窗 |
| `NEWS_SCHEDULER_ENABLED` | `true` | `false` 时只提供页面与 API |
| `NEWS_FETCH_TIMEOUT_SECS` / `NEWS_MAX_CONCURRENT_RUNS` | `300` / `2` | |
| `NEWS_RETENTION_DAYS` | `0` | `0` 永久保留条目 |
| `REDIS_ADDR` / `REDIS_PASS` / `REDIS_DB` | 空 / 空 / `1` | 邮件队列与缓存 |
| `SESSION_COOKIE_NAME` / `SESSION_TTL_DAYS` / `EMAIL_LOGIN_LINK_TTL_MINUTES` | `news_session` / `30` / `30` | |
| `EMAIL_STREAM_PRIORITY` / `EMAIL_OID` / `EMAIL_REPLY_TO` | `important` / `0` / 空 | 与 `~/xabcos/mail` 的契约一致 |

## 管理 API

管理页位于 `/admin/`，接口只接受同源浏览器请求：

```text
POST   /api/auth/email/start | consume | logout      GET /api/me
GET    /api/status
GET    /api/topics            POST /api/topics
PATCH  /api/topics/{id}       DELETE /api/topics/{id}
POST   /api/topics/{id}/run
GET    /api/topics/{id}/runs?limit=50
GET    /api/topics/{id}/items?day=YYYY-MM-DD
PATCH  /api/items/{id}        DELETE /api/items/{id}
```

登录发送频率限制与 broadcast 相同：同一邮箱 60 秒内不重发，10 分钟内最多 3 次，全站 60 秒内最多 120 次。

## 部署

服务监听容器内 `8788`，`docker-compose.yml` 只绑定宿主机 `127.0.0.1:8793`；把 `news.xairouter.com` 的反向代理 upstream 指向它。`.env`（`0600`）提供 `AI_API_KEY` 和 SMTP `EMAIL_PASS`：

```bash
cd /home/ywgx/1/news
printf 'AI_API_KEY=…\nEMAIL_PASS=…\n' > .env && chmod 0600 .env
docker compose config >/dev/null
docker compose up -d
```

CI 与 broadcast 相同：`quality-gate.yml` 执行 `make release-check`；`build-deploy.yml` 在 `main` push 后构建并推送 `rust`、`<date>-rust`、`<version>-rust` 标签，commit message 含 `prod` 或手动触发时部署生产；`manual-deploy-prod.yml` 直接部署当前镜像。

## 验证

```bash
make release-check   # fmt、test、clippy、两份 JS 语法、compose 配置
```

单元测试覆盖调度计算、URL 规范化与指纹、标题相似度、条目校验、真实 Responses 返回样例（`tests/fixtures/`）的解析、ETag/304。
