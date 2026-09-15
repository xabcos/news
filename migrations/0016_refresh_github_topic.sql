UPDATE topics
SET description = 'GitHub 与 X 热点 AI 项目及开源生态',
    brief = $$中文：关注 GitHub 热门 AI 项目、开源模型、开发框架、Agent 工具、开发者基础设施和社区趋势，以及 xairouter.com、daily.dev、github.com/HackerNews/API 的开发者最新动态。跟踪 AWS Bedrock、OpenAI、Anthropic、智谱、DeepSeek、Kimi、Qwen、MiniMax、Meta 等生态；关注 X 平台 @github、@CodexReleases、@ClaudeDevs、@AnthropicAI、@GHCopilotLog、@ClaudeCodeLog、@moonbitlang、@opencode、@zcode_ai、@dotey、@cursor_ai、@Cloudflare、@arena、@OpenRouter、@Docker、@deepseek_ai 等账号。优先项目仓库、官方文档、发布记录和真实使用反馈，结合 GitHub 活跃度与 X 传播证据判断热度，排除刷榜、营销和未经证实的传闻。
English: Track popular AI projects on GitHub, open-source models, developer frameworks, agent tools, developer infrastructure and community trends. Follow developer updates from xairouter.com, daily.dev and github.com/HackerNews/API; the ecosystems around AWS Bedrock, OpenAI, Anthropic, Zhipu, DeepSeek, Kimi, Qwen, MiniMax and Meta; and X accounts @github, @CodexReleases, @ClaudeDevs, @AnthropicAI, @GHCopilotLog, @ClaudeCodeLog, @moonbitlang, @opencode, @zcode_ai, @dotey, @cursor_ai, @Cloudflare, @arena, @OpenRouter, @Docker and @deepseek_ai. Prefer repository pages, official documentation, release notes and real usage evidence. Judge project heat using verifiable GitHub activity and X propagation; exclude artificial rankings, marketing and unverified claims.$$,
    updated_at = NOW()
WHERE id = 'github'
  AND owner_user_id IS NULL;
