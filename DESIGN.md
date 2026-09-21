# NEWS 设计说明

## 产品边界

NEWS 负责公共和个人新闻主题、定时抓取、去重、摘要、图片新闻、每日 XAI 日报、账户和支付。浏览器不持有 AI 密钥；匿名请求只读取公共内容，登录请求由服务端按账户和角色授权。

## 数据模型

核心表为 `users`、`sessions`、`email_login_tokens`、`topics`、`runs`、`items`、`ai_insights`、`plans`、`orders`、`payment_events`、`maintenance_state` 和 `user_metrics`。`ai_insights.briefing_day` 标记本地日报日期，生成结果的公开分析、公众号稿和 X Articles 稿保存在 payload 中。主题通过 `owner_user_id` 区分公共和个人数据；新闻条目按主题、日期和 URL 指纹去重；订单保存创建时的价格和套餐条款。

公共主题按职责分为跨行业概览、视觉内容、AI 圈热议、中国 AI、OpenAI、Anthropic、GitHub 开源、全球事件、应用案例和 Codex 运维信号。`/insight` 提供 XAI 日报：默认每天北京时间06:30补采公共栏目，再生成前24小时新闻和最近7天趋势。同一份结构化热点、传闻状态、图片选择及分析用于公开页面、公众号 Markdown 和 X Articles；同一天只保留最后一次成功生成。专门主题承接自己的来源和事件，跨行业主题避免重复收录。

## 路由边界

公共读取包括 `/digest`、`/topics`、`/days`、`/insight` 和 `/feed.xml`。账户路由位于 `/api/auth/*`、`/api/me/*`，管理员路由位于 `/api/admin/*`；管理员可通过 `/api/admin/insight` 查看或重新生成最近一期已截止的 XAI 日报稿件。支付使用 `/api/plans`、`/api/orders*` 和 `/api/webhooks/payment`。未知 `/api` 路由统一返回 JSON 404，静态页面由服务端托管。

## 调度和数据流

调度器每 30 秒使用数据库锁认领到期主题，进程内集合防止同一主题重复运行，信号量限制并发。抓取流程为：

```text
Responses API(web_search + x_search)
        ↓
URL、时间、长度和标签校验
        ↓
URL 指纹和标题相似度去重
        ↓
PostgreSQL 事务写入 items 和 runs
        ↓
刷新 Redis、内存快照和 ETag
```

图片转存放在事务外，避免慢上游长时间占用数据库连接。日报协调任务不占用信号量，补采与生成依次使用同一并发额度；成功补采按 `runs.window_end` 记录，重试只补失败栏目。`ai_insights.window_start/window_end` 固定24小时新闻区间，重试、延迟和手动重生成均不移动截止点。7天趋势按发布时间取原始新闻，统计按 URL 指纹去重并排除传闻、图片帖和无明确日期的条目；模型材料按日、栏目均衡抽样。成功结果写入 `ai_insights`，生成期间保留旧成功稿；后台任务在关闭时先取消外部请求，再等待有限的清理窗口。

配图候选优先来自 `p`，同时预留普通新闻图片；最多12张真实图片输入模型，选定封面与正文图片后按内容哈希和原始地址去重。程序统一输出稿件章节和参考来源；X 的剪贴板同时写入 HTML 与纯文本，正文使用列表项而非空段落。旧稿通过可选字段继续读取。

## 缓存和 HTTP

读取顺序为进程内存、Redis、PostgreSQL，写入顺序为数据库、Redis、内存。公共响应使用稳定生成时间和 ETag；内容更新、日切、主题变更和清理会失效缓存。工作台响应始终 `no-store`。服务设置 CORS、CSP、HSTS、同源来源校验、请求大小和路径长度限制。

## 账户和支付

登录使用邮箱一次性链接，服务端只保存令牌哈希；成功登录后使用 HttpOnly 会话 cookie，另用角色 cookie 让前端选择公共或私有 feed。管理员邮箱由环境变量指定，其他账户默认使用 Free。Pro 订单固定金额和期限，YPay 回调经过 HMAC 校验后幂等开通权益；未支付订单会自动关单和清理。

## 部署和初始数据库

应用监听容器内 `8788`，Compose 只绑定宿主机回环地址，由 Nginx 提供域名入口。启动时按顺序执行 `migrations/` 中的迁移，创建 NEWS schema、套餐目录和公共主题目录。

## 验证

`make release-check` 覆盖格式、单元测试、Clippy、前端语法和 Compose 配置。部署后检查迁移记录、公共 API、登录、工作台、支付回调和调度日志，确认数据库只包含 NEWS 核心表。
