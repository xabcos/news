//! Codex & Claude: a handbook on Claude Code and Codex in sixteen chapters and four groups, each
//! an article XABC keeps in step with the official documentation and what developers share
//! (`xabc::handbook`). The topic `agents` collects that news; its tab leads to the handbook's
//! page: the contents, the latest news and the official documentation. A chapter reads like an
//! article, with the contents beside it on a wide screen; at the same address with `.md` it is
//! Markdown, which its 「复制页面」 copies for a reader to hand to an agent and agents read.
use super::{
    image_id, missing,
    pages::{Mode, render},
};
use crate::{
    digest::{Rendered, conditional},
    error::Result,
    reading::{self, Reader},
    site::{Chrome, absolute, choose, escape, localized},
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, header},
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

/// The topic whose news the chapters are brought up to date from, and whose tab leads here.
pub(crate) const TOPIC: &str = "agents";
/// Where the handbook is, a chapter at `{PATH}/{key}` and as Markdown at `{PATH}/{key}.md`.
pub(crate) const PATH: &str = "/agents";
/// The news the handbook's page shows, the newest first.
const NEWS: i64 = 10;

/// The handbook's name, the same in both languages.
pub(crate) fn label(_en: bool) -> &'static str {
    "Codex & Claude"
}

/// The chapters' groups, in Chinese and English.
const GROUPS: [(&str, &str); 4] = [
    ("上手与选型", "Getting started"),
    ("配置与扩展", "Configure and extend"),
    ("开发工作流", "Workflows"),
    ("API 与 SDK", "API and SDK"),
];

/// A chapter: its key in addresses and in `articles.chapter`, its group, its title and what it
/// covers; the official pages it rests on, whose text the writer is given and whose changes
/// bring it up to date; and words a story on its subject holds.
pub(crate) struct Chapter {
    pub(crate) key: &'static str,
    group: usize,
    pub(crate) title: &'static str,
    pub(crate) about: &'static str,
    pub(crate) pages: &'static [&'static str],
    pub(crate) words: &'static [&'static str],
}

impl Chapter {
    /// The chapter's group in the page's language; the writer reads it in Chinese.
    pub(crate) fn group(&self, en: bool) -> &'static str {
        let (zh, english) = GROUPS[self.group];
        choose(en, zh, english)
    }

    pub(crate) fn path(&self) -> String {
        format!("{PATH}/{}", self.key)
    }
}

/// The handbook in reading order.
pub(crate) const CHAPTERS: [Chapter; 16] = [
    Chapter {
        key: "start",
        group: 0,
        title: "上手、安装与登录",
        about: "CLI、IDE、桌面与云端怎么选，安装、登录、升级与排错",
        pages: &[
            "https://code.claude.com/docs/en/overview.md",
            "https://code.claude.com/docs/en/quickstart.md",
            "https://code.claude.com/docs/en/setup.md",
            "https://code.claude.com/docs/en/authentication.md",
            "https://code.claude.com/docs/en/platforms.md",
            "https://learn.chatgpt.com/docs/quickstart.md",
            "https://learn.chatgpt.com/docs/codex/cli.md",
            "https://learn.chatgpt.com/docs/auth.md",
            "https://learn.chatgpt.com/docs/codex/ide.md",
        ],
        words: &[
            "安装",
            "install",
            "登录",
            "login",
            "升级",
            "upgrade",
            "VS Code",
            "JetBrains",
            "桌面版",
        ],
    },
    Chapter {
        key: "models",
        group: 0,
        title: "模型与思考强度",
        about: "当前模型怎么选，思考强度与快速模式，模型别名、版本与迁移",
        pages: &[
            "https://code.claude.com/docs/en/model-config.md",
            "https://code.claude.com/docs/en/fast-mode.md",
            "https://code.claude.com/docs/en/advisor.md",
            "https://platform.claude.com/docs/en/about-claude/models/choosing-a-model.md",
            "https://platform.claude.com/docs/en/about-claude/models/model-ids-and-versions.md",
            "https://platform.claude.com/docs/en/about-claude/models/migration-guide.md",
            "https://learn.chatgpt.com/docs/models.md",
            "https://learn.chatgpt.com/docs/model-selection.md",
            "https://learn.chatgpt.com/docs/agent-configuration/speed.md",
            "https://developers.openai.com/api/docs/guides/model-selection.md",
        ],
        words: &[
            "Fable",
            "Opus",
            "Sonnet",
            "Haiku",
            "GPT-6",
            "/model",
            "思考强度",
            "reasoning effort",
            "fast mode",
            "快速模式",
        ],
    },
    Chapter {
        key: "costs",
        group: 0,
        title: "额度、计费与成本",
        about: "订阅额度与 API 计费的区别，缓存与上下文的成本，省额度的做法",
        pages: &[
            "https://code.claude.com/docs/en/costs.md",
            "https://code.claude.com/docs/en/prompt-caching.md",
            "https://learn.chatgpt.com/docs/pricing.md",
        ],
        words: &[
            "额度",
            "用量",
            "限额",
            "usage limit",
            "计费",
            "billing",
            "账单",
            "成本",
            "prompt cach",
            "缓存命中",
            "/cost",
            "/usage",
        ],
    },
    Chapter {
        key: "context",
        group: 1,
        title: "CLAUDE.md、AGENTS.md 与上下文",
        about: "项目说明怎么写、放在哪，记忆、会话、压缩与上下文管理",
        pages: &[
            "https://code.claude.com/docs/en/memory.md",
            "https://code.claude.com/docs/en/claude-directory.md",
            "https://code.claude.com/docs/en/context-window.md",
            "https://code.claude.com/docs/en/sessions.md",
            "https://learn.chatgpt.com/docs/agent-configuration/agents-md.md",
            "https://learn.chatgpt.com/docs/customization/memories.md",
        ],
        words: &[
            "CLAUDE.md",
            "AGENTS.md",
            "记忆",
            "上下文",
            "context window",
            "/compact",
            "压缩",
            "/resume",
            "--continue",
        ],
    },
    Chapter {
        key: "settings",
        group: 1,
        title: "settings.json 与 config.toml",
        about: "配置文件在哪、谁覆盖谁，常用配置项、最小模板与排错",
        pages: &[
            "https://code.claude.com/docs/en/settings.md",
            "https://code.claude.com/docs/en/settings-example.md",
            "https://code.claude.com/docs/en/env-vars.md",
            "https://code.claude.com/docs/en/debug-your-config.md",
            "https://code.claude.com/docs/en/settings-reference.md",
            "https://learn.chatgpt.com/docs/config-file/config-basic.md",
            "https://learn.chatgpt.com/docs/config-file/config-advanced.md",
            "https://learn.chatgpt.com/docs/config-file/config-sample.md",
            "https://learn.chatgpt.com/docs/config-file/environment-variables.md",
            "https://learn.chatgpt.com/docs/config-file/config-reference.md",
        ],
        words: &[
            "settings.json",
            "config.toml",
            "配置文件",
            "profile",
            "环境变量",
            "managed settings",
        ],
    },
    Chapter {
        key: "permissions",
        group: 1,
        title: "权限与沙箱",
        about: "权限模式与审批，允许与禁止规则，沙箱、网络与组织策略",
        pages: &[
            "https://code.claude.com/docs/en/permissions.md",
            "https://code.claude.com/docs/en/permission-modes.md",
            "https://code.claude.com/docs/en/auto-mode-config.md",
            "https://code.claude.com/docs/en/sandboxing.md",
            "https://code.claude.com/docs/en/security.md",
            "https://learn.chatgpt.com/docs/permission-modes.md",
            "https://learn.chatgpt.com/docs/permissions.md",
            "https://learn.chatgpt.com/docs/sandboxing.md",
            "https://learn.chatgpt.com/docs/agent-approvals-security.md",
            "https://learn.chatgpt.com/docs/agent-configuration/rules.md",
        ],
        words: &[
            "权限",
            "permission",
            "沙箱",
            "sandbox",
            "deny",
            "审批",
            "approval",
            "auto mode",
            "自动模式",
            "dangerously",
            "yolo",
        ],
    },
    Chapter {
        key: "mcp",
        group: 1,
        title: "MCP",
        about: "接入 MCP 服务器：作用域与认证，常用服务器，工具与上下文的开销",
        pages: &[
            "https://code.claude.com/docs/en/mcp-quickstart.md",
            "https://code.claude.com/docs/en/mcp.md",
            "https://learn.chatgpt.com/docs/extend/mcp.md",
        ],
        words: &["MCP"],
    },
    Chapter {
        key: "extensions",
        group: 1,
        title: "Skills、插件与命令",
        about: "Skills、插件、斜杠命令与自定义提示词：怎么选、怎么装、怎么写",
        pages: &[
            "https://code.claude.com/docs/en/features-overview.md",
            "https://code.claude.com/docs/en/skills.md",
            "https://code.claude.com/docs/en/plugins/overview.md",
            "https://code.claude.com/docs/en/plugins/install.md",
            "https://code.claude.com/docs/en/commands.md",
            "https://learn.chatgpt.com/docs/skills-and-plugins.md",
            "https://learn.chatgpt.com/docs/build-skills.md",
            "https://learn.chatgpt.com/docs/plugins.md",
            "https://learn.chatgpt.com/docs/custom-prompts.md",
        ],
        words: &[
            "skill",
            "插件",
            "plugin",
            "marketplace",
            "斜杠命令",
            "slash command",
            "自定义命令",
            "custom prompt",
        ],
    },
    Chapter {
        key: "hooks",
        group: 1,
        title: "Hooks",
        about: "生命周期事件，输入输出与退出码，可以直接用的实用钩子",
        pages: &[
            "https://code.claude.com/docs/en/hooks-guide.md",
            "https://code.claude.com/docs/en/hooks.md",
            "https://learn.chatgpt.com/docs/hooks.md",
            "https://learn.chatgpt.com/docs/notifications.md",
        ],
        words: &[
            "hook",
            "钩子",
            "PreToolUse",
            "PostToolUse",
            "SessionStart",
            "PreCompact",
            "UserPromptSubmit",
            "notify",
        ],
    },
    Chapter {
        key: "workflow",
        group: 2,
        title: "日常开发、评审与测试",
        about: "官方最佳实践：探索、计划、实现、验证，代码评审与测试",
        pages: &[
            "https://code.claude.com/docs/en/best-practices.md",
            "https://code.claude.com/docs/en/common-workflows.md",
            "https://code.claude.com/docs/en/how-claude-code-works.md",
            "https://code.claude.com/docs/en/code-review-local.md",
            "https://code.claude.com/docs/en/checkpointing.md",
            "https://learn.chatgpt.com/docs/prompting.md",
            "https://learn.chatgpt.com/docs/working-with-agents.md",
            "https://learn.chatgpt.com/docs/code-review.md",
        ],
        words: &[
            "最佳实践",
            "best practice",
            "plan mode",
            "计划模式",
            "/review",
            "代码审查",
            "code review",
            "TDD",
            "工作流",
            "workflow",
            "提示词",
        ],
    },
    Chapter {
        key: "parallel",
        group: 2,
        title: "大型仓库、长任务与并行",
        about: "子代理、并行会话与 worktree，长任务与大型代码库",
        pages: &[
            "https://code.claude.com/docs/en/sub-agents.md",
            "https://code.claude.com/docs/en/agents.md",
            "https://code.claude.com/docs/en/agent-teams.md",
            "https://code.claude.com/docs/en/worktrees.md",
            "https://code.claude.com/docs/en/large-codebases.md",
            "https://learn.chatgpt.com/docs/agent-configuration/subagents.md",
            "https://learn.chatgpt.com/docs/environments/git-worktrees.md",
            "https://learn.chatgpt.com/docs/long-running-work.md",
        ],
        words: &[
            "subagent",
            "子代理",
            "子智能体",
            "worktree",
            "并行",
            "parallel",
            "agent team",
            "多智能体",
            "multi-agent",
            "长任务",
            "monorepo",
            "大型代码库",
        ],
    },
    Chapter {
        key: "remote",
        group: 2,
        title: "远程、云端与多端",
        about: "云端会话、远程控制、手机与浏览器，在桌面、IDE 与云端之间接力",
        pages: &[
            "https://code.claude.com/docs/en/remote-control.md",
            "https://code.claude.com/docs/en/claude-code-on-the-web.md",
            "https://code.claude.com/docs/en/mobile.md",
            "https://code.claude.com/docs/en/desktop.md",
            "https://code.claude.com/docs/en/claude-projects.md",
            "https://learn.chatgpt.com/docs/cloud.md",
            "https://learn.chatgpt.com/docs/environments/cloud-environments.md",
            "https://learn.chatgpt.com/docs/remote-connections.md",
            "https://learn.chatgpt.com/docs/mobile.md",
        ],
        words: &[
            "Remote Control",
            "远程",
            "云端",
            "on the web",
            "手机",
            "mobile",
            "iOS",
            "Android",
            "Codex Cloud",
        ],
    },
    Chapter {
        key: "automation",
        group: 2,
        title: "无头模式与 CI",
        about: "在脚本与 CI 里调用：结构化输出、GitHub Actions 与定时任务",
        pages: &[
            "https://code.claude.com/docs/en/headless.md",
            "https://code.claude.com/docs/en/cli-reference.md",
            "https://code.claude.com/docs/en/github-actions.md",
            "https://code.claude.com/docs/en/routines.md",
            "https://code.claude.com/docs/en/scheduled-tasks.md",
            "https://learn.chatgpt.com/docs/non-interactive-mode.md",
            "https://learn.chatgpt.com/docs/developer-commands.md?surface=cli",
            "https://learn.chatgpt.com/docs/github-action.md",
            "https://learn.chatgpt.com/docs/automations.md",
        ],
        words: &[
            "claude -p",
            "headless",
            "无头",
            "codex exec",
            "GitHub Action",
            "定时任务",
            "scheduled task",
            "routine",
            "--output-format",
            "非交互",
        ],
    },
    Chapter {
        key: "claude-api",
        group: 3,
        title: "Claude API",
        about: "Messages API 的请求与流式，工具调用、结构化输出、思考、缓存、批处理与错误",
        pages: &[
            "https://platform.claude.com/docs/en/api/overview.md",
            "https://platform.claude.com/docs/en/build-with-claude/working-with-messages.md",
            "https://platform.claude.com/docs/en/agents-and-tools/tool-use/overview.md",
            "https://platform.claude.com/docs/en/build-with-claude/streaming.md",
            "https://platform.claude.com/docs/en/build-with-claude/structured-outputs.md",
            "https://platform.claude.com/docs/en/build-with-claude/extended-thinking.md",
            "https://platform.claude.com/docs/en/build-with-claude/effort.md",
            "https://platform.claude.com/docs/en/build-with-claude/prompt-caching.md",
            "https://platform.claude.com/docs/en/build-with-claude/batch-processing.md",
            "https://platform.claude.com/docs/en/api/errors.md",
            "https://platform.claude.com/docs/en/api/rate-limits.md",
        ],
        words: &[
            "Claude API",
            "Messages API",
            "Anthropic API",
            "tool use",
            "工具调用",
            "prompt caching",
            "Batch API",
            "structured output",
            "结构化输出",
            "extended thinking",
        ],
    },
    Chapter {
        key: "openai-api",
        group: 3,
        title: "OpenAI API",
        about: "Responses API 的请求与会话状态，函数调用、结构化输出、流式、推理与错误",
        pages: &[
            "https://developers.openai.com/api/docs/quickstart.md",
            "https://developers.openai.com/api/docs/guides/text.md",
            "https://developers.openai.com/api/docs/guides/conversation-state.md",
            "https://developers.openai.com/api/docs/guides/function-calling.md",
            "https://developers.openai.com/api/docs/guides/structured-outputs.md",
            "https://developers.openai.com/api/docs/guides/streaming-responses.md",
            "https://developers.openai.com/api/docs/guides/reasoning.md",
            "https://developers.openai.com/api/docs/guides/prompt-caching.md",
            "https://developers.openai.com/api/docs/guides/error-codes.md",
            "https://developers.openai.com/api/docs/guides/rate-limits.md",
            "https://developers.openai.com/api/docs/guides/migrate-to-responses.md",
        ],
        words: &[
            "OpenAI API",
            "Responses API",
            "Chat Completions",
            "function calling",
            "函数调用",
            "structured outputs",
            "Realtime API",
            "previous_response_id",
        ],
    },
    Chapter {
        key: "sdk",
        group: 3,
        title: "Agent SDK",
        about: "把智能体嵌进自己的程序：Claude Agent SDK、Codex SDK 与 App Server、OpenAI Agents SDK",
        pages: &[
            "https://code.claude.com/docs/en/agent-sdk/overview.md",
            "https://code.claude.com/docs/en/agent-sdk/quickstart.md",
            "https://code.claude.com/docs/en/agent-sdk/configuration.md",
            "https://learn.chatgpt.com/docs/codex-sdk.md",
            "https://learn.chatgpt.com/docs/app-server.md",
            "https://developers.openai.com/api/docs/guides/agents/sdk.md",
        ],
        words: &[
            "Agent SDK",
            "Agents SDK",
            "Codex SDK",
            "App Server",
            "claude-agent-sdk",
            "app-server",
        ],
    },
];

pub(crate) fn chapter(key: &str) -> Option<&'static Chapter> {
    CHAPTERS.iter().find(|chapter| chapter.key == key)
}

/// Where readers read an official page whose Markdown twin is registered, and its Chinese
/// version where Anthropic writes one: the address without `.md`, `/docs/en/` as `/docs/zh-CN/`.
pub(crate) fn readable(page: &str) -> [String; 2] {
    let page = match page.split_once('?') {
        Some((path, query)) => format!("{}?{query}", path.trim_end_matches(".md")),
        None => page.trim_end_matches(".md").to_owned(),
    };
    let chinese = page.replace("/docs/en/", "/docs/zh-CN/");
    [page, chinese]
}

/// A link's words in Chinese and English, and where it leads.
type Link = (&'static str, &'static str, &'static str);

/// The official documentation the handbook's page leads to, readers' pages in Chinese where
/// there are.
const OFFICIAL: [(&str, &[Link]); 5] = [
    (
        "Claude Code",
        &[
            (
                "中文文档",
                "Docs",
                "https://code.claude.com/docs/zh-CN/overview",
            ),
            (
                "更新日志",
                "Changelog",
                "https://code.claude.com/docs/en/changelog",
            ),
            (
                "每周更新",
                "What's new",
                "https://code.claude.com/docs/zh-CN/whats-new",
            ),
        ],
    ),
    (
        "Claude API",
        &[
            (
                "中文文档",
                "Docs",
                "https://platform.claude.com/docs/zh-CN/intro",
            ),
            (
                "模型",
                "Models",
                "https://platform.claude.com/docs/zh-CN/models/overview",
            ),
            (
                "发布说明",
                "Release notes",
                "https://platform.claude.com/docs/zh-CN/release-notes/overview",
            ),
        ],
    ),
    (
        "Codex",
        &[
            ("文档", "Docs", "https://learn.chatgpt.com/docs"),
            (
                "每周更新",
                "What's new",
                "https://learn.chatgpt.com/docs/whats-new",
            ),
            (
                "版本发布",
                "Releases",
                "https://github.com/openai/codex/releases",
            ),
        ],
    ),
    (
        "OpenAI API",
        &[
            ("文档", "Docs", "https://developers.openai.com/api/docs"),
            (
                "模型",
                "Models",
                "https://developers.openai.com/api/docs/models",
            ),
        ],
    ),
    ("𝑳𝑳𝑴", &[("价格与规格", "Prices and specs", "/llm")]),
];

/// The handbook's page.
const HUB: Chrome<'static> = Chrome {
    current: TOPIC,
    section: "reads handbook-hub",
    stylesheet: "/reads.css?v=24",
    script: "/reads.js?v=12",
    feed: ("Codex & Claude", "/feed.xml?topic=agents"),
};

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route(PATH, get(hub))
            .route("/agents/{chapter}", get(read)),
    )
}

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
}

/// The chapters readers can read, by key, with when each was last brought up to date.
pub(super) async fn shown(state: &AppState) -> Result<HashMap<String, DateTime<Utc>>> {
    Ok(sqlx::query_as::<_, (String, DateTime<Utc>)>(
        "SELECT a.chapter, a.updated_at FROM articles a JOIN users u ON u.id = a.author_id
         WHERE a.chapter IS NOT NULL AND a.status = 'published' AND u.status",
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect())
}

/// The contents beside a chapter on a wide screen: every chapter readers can read, by group,
/// `current` marked.
pub(super) fn contents(shown: &HashMap<String, DateTime<Utc>>, current: &str, en: bool) -> String {
    let mut nav = format!(
        "<nav class=handbook-side aria-label=\"{}\"><a class=handbook-home href=\"{}\">{}</a>",
        choose(en, "手册目录", "Contents"),
        localized(PATH, en),
        escape(label(en))
    );
    for (group, (zh, english)) in GROUPS.iter().enumerate() {
        let chapters: Vec<&Chapter> = CHAPTERS
            .iter()
            .filter(|chapter| chapter.group == group && shown.contains_key(chapter.key))
            .collect();
        if chapters.is_empty() {
            continue;
        }
        let _ = write!(nav, "<h2>{}</h2><ul>", choose(en, zh, english));
        for chapter in chapters {
            let _ = write!(
                nav,
                "<li><a href=\"{}\"{}>{}</a></li>",
                localized(&chapter.path(), en),
                if chapter.key == current {
                    " aria-current=\"page\""
                } else {
                    ""
                },
                escape(chapter.title)
            );
        }
        nav.push_str("</ul>");
    }
    nav.push_str("</nav>");
    nav
}

/// The links under a chapter: the chapters before and after it that readers can read.
pub(super) fn steps(shown: &HashMap<String, DateTime<Utc>>, current: &str, en: bool) -> String {
    let readable: Vec<&Chapter> = CHAPTERS
        .iter()
        .filter(|chapter| shown.contains_key(chapter.key))
        .collect();
    let Some(at) = readable.iter().position(|chapter| chapter.key == current) else {
        return String::new();
    };
    let link = |chapter: Option<&&Chapter>, rel: &str, arrow: (&str, &str)| {
        chapter
            .map(|chapter| {
                format!(
                    "<a rel={rel} href=\"{}\">{}{}{}</a>",
                    localized(&chapter.path(), en),
                    arrow.0,
                    escape(chapter.title),
                    arrow.1
                )
            })
            .unwrap_or_default()
    };
    format!(
        "<nav class=handbook-steps>{}{}</nav>",
        link(
            at.checked_sub(1).and_then(|at| readable.get(at)),
            "prev",
            ("← ", "")
        ),
        link(readable.get(at + 1), "next", ("", " →"))
    )
}

/// A piece of news on the handbook's page.
#[derive(sqlx::FromRow)]
struct News {
    title: String,
    title_en: String,
    url: String,
    tags: Vec<String>,
    tags_en: Vec<String>,
    at: DateTime<Utc>,
}

async fn hub(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    reader.check(&state, TOPIC)?;
    let en = params.lang.as_deref() == Some("en");
    let shown = shown(&state).await?;
    let news: Vec<News> = sqlx::query_as(
        "SELECT title, title_en, url, tags, tags_en, COALESCE(published_at, first_seen_at) AS at
         FROM items WHERE topic_id = $1 AND NOT hidden
         ORDER BY at DESC, id DESC LIMIT $2",
    )
    .bind(TOPIC)
    .bind(NEWS)
    .fetch_all(&state.pool)
    .await?;
    let zone = state.config.timezone;
    let mut html = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p></header>",
        escape(label(en)),
        choose(
            en,
            "Claude Code 与 Codex 的中文实用手册：以官方文档为准，汇集官方与网友的最佳实践和实用配置，官方一更新就跟着改。",
            "A handbook on Claude Code and Codex, kept to the official documentation: the vendors' and developers' best practices and working configurations, updated as the documentation changes.",
        ),
    );
    if shown.is_empty() {
        let _ = write!(
            html,
            "<p class=muted>{}</p>",
            choose(
                en,
                "手册正在编写，各章写好就会出现在这里。",
                "The handbook is being written; its chapters appear here as they are done.",
            )
        );
    }
    html.push_str("<div class=handbook-groups>");
    for (group, (zh, english)) in GROUPS.iter().enumerate() {
        let mut rows = String::new();
        for chapter in CHAPTERS.iter().filter(|chapter| chapter.group == group) {
            let Some(updated) = shown.get(chapter.key) else {
                continue;
            };
            let _ = write!(
                rows,
                "<li><a href=\"{}\">{}</a><time datetime=\"{}\">{}</time><p>{}</p></li>",
                localized(&chapter.path(), en),
                escape(chapter.title),
                updated.to_rfc3339(),
                updated.with_timezone(&zone).format("%m-%d"),
                escape(chapter.about)
            );
        }
        if !rows.is_empty() {
            let _ = write!(
                html,
                "<section class=handbook-group><h2>{}</h2><ol>{rows}</ol></section>",
                choose(en, zh, english)
            );
        }
    }
    html.push_str("</div>");
    if !news.is_empty() {
        let _ = write!(
            html,
            "<section class=handbook-news><h2>{}<a href=\"{}\">{}</a></h2><ol>",
            choose(en, "最新动态", "Latest"),
            escape(&localized(&format!("/?topic={TOPIC}"), en)),
            choose(en, "全部 →", "All →")
        );
        for item in &news {
            let (title, tags) = if en && !item.title_en.is_empty() {
                (&item.title_en, &item.tags_en)
            } else {
                (&item.title, &item.tags)
            };
            let _ = write!(
                html,
                "<li><time datetime=\"{}\">{}</time><a href=\"{}\" target=_blank rel=\"noopener noreferrer\">{}</a>{}</li>",
                item.at.to_rfc3339(),
                item.at.with_timezone(&zone).format("%m-%d"),
                escape(&item.url),
                escape(title),
                tags.first()
                    .map(|tag| format!("<span>{}</span>", escape(tag)))
                    .unwrap_or_default()
            );
        }
        html.push_str("</ol></section>");
    }
    let _ = write!(
        html,
        "<section class=handbook-docs><h2>{}</h2><dl>",
        choose(en, "官方文档", "Official documentation")
    );
    for (name, links) in OFFICIAL {
        let links: Vec<String> = links
            .iter()
            .map(|(zh, english, href)| {
                format!(
                    "<a href=\"{}\"{}>{}</a>",
                    escape(&localized(href, en && href.starts_with('/'))),
                    if href.starts_with('/') {
                        ""
                    } else {
                        " target=_blank rel=\"noopener noreferrer\""
                    },
                    choose(en, zh, english)
                )
            })
            .collect();
        let _ = write!(html, "<dt>{name}</dt><dd>{}</dd>", links.join(" · "));
    }
    html.push_str("</dl></section>");
    let page = crate::site::layout(
        &state,
        en,
        PATH,
        &format!(
            "{} · {}",
            label(en),
            choose(en, "Claude Code 与 Codex 中文实用手册", "Handbook")
        ),
        choose(
            en,
            "以官方文档为准的 Claude Code 与 Codex 中文实用手册：上手与选型、配置与扩展、开发工作流、API 与 SDK，附最新动态与官方文档。",
            "A handbook on Claude Code and Codex kept to the official documentation, with the latest news.",
        ),
        &format!("<div class=handbook>{html}</div>"),
        None,
        &HUB,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(page),
        "text/html; charset=utf-8",
    ))
}

/// A chapter: its page, or at its address with `.md` its Markdown.
async fn read(
    State(state): State<AppState>,
    reader: Reader,
    Path(key): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let (chapter, twin) = match key.strip_suffix(".md") {
        Some(key) => (chapter(key), true),
        None => (chapter(&key), false),
    };
    let chapter = chapter.ok_or_else(missing)?;
    // As the page, its Markdown is read while the topic is shown; an administrator reads both
    // whatever the topic does.
    if twin {
        if !reader.is_admin() {
            reader.check(&state, TOPIC)?;
        }
        return markdown(&state, chapter, reader.is_admin(), &headers).await;
    }
    let id: Uuid = sqlx::query_scalar("SELECT id FROM articles WHERE chapter = $1")
        .bind(chapter.key)
        .fetch_optional(&state.pool)
        .await?
        .ok_or_else(missing)?;
    let mode = if reader.is_admin() {
        Mode::Review
    } else {
        Mode::Public
    };
    let html = render(&state, id, params.lang.as_deref() == Some("en"), mode).await?;
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// A chapter as Markdown: its title, where it is and when it was last brought up to date, and
/// its body, its own pictures at their addresses. Its page, not this, is for search engines;
/// an administrator also reads a chapter that waits for a ❤️.
async fn markdown(
    state: &AppState,
    chapter: &Chapter,
    admin: bool,
    headers: &HeaderMap,
) -> Result<Response> {
    let (id, body, updated_at): (Uuid, String, DateTime<Utc>) = sqlx::query_as(
        "SELECT a.id, a.body, a.updated_at FROM articles a JOIN users u ON u.id = a.author_id
         WHERE a.chapter = $1 AND u.status
           AND (a.status = 'published' OR ($2 AND a.status = 'pending'))",
    )
    .bind(chapter.key)
    .bind(admin)
    .fetch_optional(&state.pool)
    .await?
    .ok_or_else(missing)?;
    let images: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, url FROM article_images WHERE article_id = $1 AND expires_at > now()",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect();
    let text = format!(
        "# {}\n\n> 来源：{} · 更新于 {}\n\n{}\n",
        chapter.title,
        absolute(state, &chapter.path()),
        updated_at
            .with_timezone(&state.config.timezone)
            .format("%Y-%m-%d"),
        with_pictures(&body, &images).trim()
    );
    let mut response = conditional(
        headers,
        &Rendered::new(text),
        "text/markdown; charset=utf-8",
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-robots-tag"),
        HeaderValue::from_static("noindex"),
    );
    Ok(response)
}

/// The body with each of its own pictures, `image:<id>`, at its address; one tmpfile no longer
/// keeps is left as it is.
fn with_pictures(body: &str, images: &HashMap<Uuid, String>) -> String {
    let mut text = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("](image:") {
        let (before, after) = rest.split_at(at + 2);
        text.push_str(before);
        let end = after.find(')').unwrap_or(after.len());
        match image_id(&after[..end]).and_then(|id| images.get(&id)) {
            Some(url) => text.push_str(url),
            None => text.push_str(&after[..end]),
        }
        rest = &after[end..];
    }
    text.push_str(rest);
    text
}

/// The handbook's addresses for the sitemap while readers can see it, with when each chapter
/// was last brought up to date.
pub(super) async fn listed(state: &AppState) -> Result<Vec<(String, Option<DateTime<Utc>>)>> {
    if Reader::Public.check(state, TOPIC).is_err() {
        return Ok(Vec::new());
    }
    let shown = shown(state).await?;
    Ok(std::iter::once((PATH.to_owned(), None))
        .chain(
            CHAPTERS
                .iter()
                .filter_map(|chapter| Some((chapter.path(), Some(*shown.get(chapter.key)?)))),
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_chapter_has_its_own_key_a_group_and_official_markdown_pages() {
        let mut keys = std::collections::HashSet::new();
        for chapter in &CHAPTERS {
            assert!(keys.insert(chapter.key), "{}", chapter.key);
            assert!(chapter.group < GROUPS.len());
            assert!(!chapter.pages.is_empty() && !chapter.words.is_empty());
            for page in chapter.pages {
                let url = url::Url::parse(page).unwrap();
                assert_eq!(url.scheme(), "https");
                assert!(url.path().ends_with(".md"), "{page}");
                assert!(
                    [
                        "https://code.claude.com/docs/en/",
                        "https://learn.chatgpt.com/docs/",
                        "https://platform.claude.com/docs/en/",
                        "https://developers.openai.com/api/docs/",
                    ]
                    .iter()
                    .any(|docs| page.starts_with(docs)),
                    "{page}"
                );
            }
        }
        // Each group has chapters, in reading order.
        let groups: Vec<usize> = CHAPTERS.iter().map(|chapter| chapter.group).collect();
        assert!(groups.is_sorted());
        assert_eq!(groups.iter().copied().max(), Some(GROUPS.len() - 1));
    }

    #[test]
    fn an_official_page_reads_without_md_and_in_chinese_where_anthropic_writes_it() {
        assert_eq!(
            readable("https://code.claude.com/docs/en/hooks.md"),
            [
                "https://code.claude.com/docs/en/hooks",
                "https://code.claude.com/docs/zh-CN/hooks"
            ]
        );
        assert_eq!(
            readable("https://learn.chatgpt.com/docs/developer-commands.md?surface=cli"),
            [
                "https://learn.chatgpt.com/docs/developer-commands?surface=cli",
                "https://learn.chatgpt.com/docs/developer-commands?surface=cli"
            ]
        );
    }

    #[test]
    fn a_chapters_markdown_links_its_own_pictures_and_leaves_others_alone() {
        let id = Uuid::new_v4();
        let gone = Uuid::new_v4();
        let images = HashMap::from([(id, "https://media.example/a.png".to_owned())]);
        assert_eq!(
            with_pictures(
                &format!("A ![图](image:{id}) B ![](image:{gone}) C [link](https://x.example)"),
                &images
            ),
            format!(
                "A ![图](https://media.example/a.png) B ![](image:{gone}) C [link](https://x.example)"
            )
        );
    }
}
