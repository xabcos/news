//! 学习: XABC's courses, one track (`TRACKS`) each. `/learn` maps the tracks, `/learn/{track}`
//! lists a track's lessons by chapter, and a lesson reads on the article page, which shows where
//! it stands (`Place`). XABC writes and revises the lessons (`xabc::lesson`).
use crate::{
    digest::{Rendered, conditional},
    error::{AppError, Result},
    reading::{self, Reader},
    site::{Chrome, choose, escape, localized},
    state::AppState,
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    routing::get,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::FromRow;
use std::{collections::HashMap, fmt::Write};
use uuid::Uuid;

/// Where the courses live.
pub(crate) const PATH: &str = "/learn";

/// The section's name, in the page's language.
pub(crate) fn label(en: bool) -> &'static str {
    choose(en, "学习", "𝑳𝒆𝒂𝒓𝒏")
}

/// The groups courses are listed in, by name and English name.
type Group = (&'static str, &'static str);
const STARTING: Group = ("AI 入门与应用", "Getting started with AI");
const BUILDING: Group = ("AI 开发", "Building with AI");
const HARDWARE: Group = ("硬件与嵌入式", "Hardware and embedded");

/// A course: its address, its group, its name, a line on what it teaches, and what XABC is told
/// about its readers, its scope and the chapters it should go through. Courses of a group stand
/// together, in the order they are listed.
pub(crate) struct Track {
    pub(crate) id: &'static str,
    group: Group,
    pub(crate) title: &'static str,
    pub(crate) title_en: &'static str,
    pub(crate) intro: &'static str,
    pub(crate) intro_en: &'static str,
    pub(crate) brief: &'static str,
}

pub(crate) const TRACKS: [Track; 9] = [
    Track {
        id: "basics",
        group: STARTING,
        title: "AI 入门",
        title_en: "AI Basics",
        intro: "从零认识大模型：它能做什么、怎么用好、怎么选",
        intro_en: "Large language models from zero: what they do, how to use them well and how to choose",
        brief: "读者是刚开始接触 AI 的普通人，不需要编程基础。建议的章节路线：大模型是什么、能做什么不能做什么 → token、上下文与幻觉 → 主流模型和产品（ChatGPT、Claude、Gemini、Grok、DeepSeek、通义千问、Kimi、豆包等）怎么选，免费与付费订阅怎么选 → 日常用法：问答、写作、翻译、总结、学习与搜索 → 多模态：看图、语音、文件与长文档 → 记忆、项目与自定义助手 → 隐私、安全与辨别 AI 内容 → 用 AI 持续学习与提效。讲概念多用生活里的类比，少用术语，用到时先解释。",
    },
    Track {
        id: "prompts",
        group: STARTING,
        title: "提示词",
        title_en: "Prompting",
        intro: "把需求说清楚：结构化提示、举例、让模型自查",
        intro_en: "Saying what you want: structured prompts, examples and self-checks",
        brief: "读者会用聊天式 AI，想让它稳定地给出好结果。建议的章节路线：好提示词的要素（目标、背景、约束、格式）→ 角色与语气 → 举例（少样本）→ 拆解复杂任务与分步推理 → 结构化输出（表格、JSON）→ 让模型自查与改写 → 长文档与资料的提问方法 → 推理模型与普通模型的提示差异 → 写作、编程、数据分析和学习等场景的提示模板 → 评测与迭代提示词，提示注入与防护。每课给出能直接复制使用的提示词，以及改进前后的对比。",
    },
    Track {
        id: "create",
        group: STARTING,
        title: "AI 创作与提效",
        title_en: "Creating with AI",
        intro: "用 AI 画图、做视频和语音，写作、办公与研究提效",
        intro_en: "Images, video and voice with AI, and faster writing, office work and research",
        brief: "读者是创作者和职场人，不需要编程。建议的章节路线：图像生成（GPT Image、Midjourney、即梦等）的提示与风格 → 图像编辑与角色一致性 → 视频生成（Sora、Veo、可灵、Grok Imagine 等）→ 语音合成、配音与音乐 → 写作与编辑 → 办公：文档、表格、演示与会议纪要 → 研究与深度搜索 → 数据分析与图表 → 学习与知识管理 → 自动化工作流 → 版权、标注与合规。工具变得快，课里写明工具的版本和日期，优先讲能迁移到别的工具的方法。",
    },
    Track {
        id: "coding",
        group: BUILDING,
        title: "AI 编程",
        title_en: "AI Coding",
        intro: "Claude Code、Codex、Cursor 等工具，从安装到专业工作流",
        intro_en: "Claude Code, Codex, Cursor and more, from setup to professional workflows",
        brief: "读者是开发者，或想用 AI 写代码的人。建议的章节路线：AI 编程工具全景与怎么选（Claude Code、OpenAI Codex、Cursor、GitHub Copilot、Gemini CLI 等）→ 安装、登录与计费方式 → 第一个项目：让 AI 读懂代码库、改代码、跑测试 → 项目说明文件（如 CLAUDE.md、AGENTS.md）与上下文管理 → 计划模式、审查改动与版本控制 → 测试驱动与调试 → 重构与大型项目 → 子代理、并行任务与自动化（hooks、自定义命令、在 CI 中运行）→ 用 MCP 接入外部工具 → 代码审查与安全 → 成本与额度控制 → 团队协作与规范。命令、配置和快捷键以各工具官方文档的最新版本为准，写明版本。",
    },
    Track {
        id: "agents",
        group: BUILDING,
        title: "Agent 与 MCP",
        title_en: "Agents & MCP",
        intro: "工具调用、MCP、自动化工作流与多 Agent",
        intro_en: "Tool calling, MCP, automated workflows and multi-agent systems",
        brief: "读者有一些编程基础。建议的章节路线：Agent 是什么、和聊天有什么不同 → 函数调用与工具调用的原理和 API 写法 → 设计好用的工具 → MCP 协议：概念与使用现成的 MCP 服务器 → 自己写 MCP 服务器（Python、TypeScript）→ 远程 MCP 与认证 → Agent 框架与 SDK（OpenAI Agents SDK、Claude Agent SDK、LangGraph 等）→ 规划、记忆与长任务 → 多 Agent 协作 → 操作浏览器与电脑 → 评测、可观测性与成本 → 安全：权限、提示注入与沙箱 → 实战项目。代码以官方 SDK 文档为准，写明版本。",
    },
    Track {
        id: "rag",
        group: BUILDING,
        title: "RAG 与知识库",
        title_en: "RAG & Knowledge Bases",
        intro: "让 AI 用上你的资料：嵌入、检索与知识库",
        intro_en: "Putting your own documents to work: embeddings, retrieval and knowledge bases",
        brief: "读者有一些编程基础。建议的章节路线：RAG 是什么、什么时候用（与长上下文、微调对比）→ 嵌入与向量相似度 → 文档解析与切分 → 向量数据库（pgvector、Milvus、Qdrant 等）与选型 → 检索：混合检索与重排 → 组装提示与引用来源 → 评测：召回率与忠实度 → 进阶：查询改写、多跳检索、GraphRAG、Agentic RAG → 多模态 RAG → 生产化：增量更新、权限、成本与延迟 → 现成方案（Dify、RAGFlow 等）与自建的取舍 → 实战项目。示例代码给出能运行的最小版本，写明库的版本。",
    },
    Track {
        id: "api",
        group: BUILDING,
        title: "模型 API 开发",
        title_en: "Model APIs",
        intro: "调用大模型 API：流式、结构化输出、缓存与成本",
        intro_en: "Building on model APIs: streaming, structured output, caching and cost",
        brief: "读者是开发者。建议的章节路线：主流 API 全景（OpenAI、Anthropic、Gemini、xAI、DeepSeek、通义千问等）与 Key 管理 → 第一次调用（Python、TypeScript、curl）→ 消息、角色与参数 → 流式输出 → 结构化输出与 JSON Schema → 工具调用 → 多模态输入：图片、PDF 与音频 → 图像、语音与嵌入接口 → 提示缓存与批处理 → token 计算、价格与成本控制（各家官方价格可以引用本站汇总的大模型价格表 https://xabcnews.com/llm）→ 错误处理、重试与限流 → 兼容接口与多模型切换 → 安全与合规 → 生产部署。接口、参数和价格以各家官方文档为准，写明日期。",
    },
    Track {
        id: "local",
        group: BUILDING,
        title: "开源与本地模型",
        title_en: "Open & Local Models",
        intro: "在自己的电脑和服务器上跑模型：部署、量化与微调",
        intro_en: "Running models on your own machines: deployment, quantization and fine-tuning",
        brief: "读者有一些命令行基础。建议的章节路线：开源模型全景（Qwen、DeepSeek、Llama、Gemma、Mistral 等）与许可证 → 硬件怎么选：显存、内存与 Apple Silicon → Ollama、LM Studio 快速上手 → llama.cpp 与 GGUF，量化的原理与选择 → vLLM、SGLang 等推理服务 → 把本地模型接入编辑器、聊天界面与 Agent → 性能调优与多卡 → 微调：LoRA、QLoRA 与数据准备 → 评测 → 部署到服务器与成本 → 端侧与移动端 → 实战项目。命令和版本以各项目官方文档为准。",
    },
    Track {
        id: "esp32",
        group: HARDWARE,
        title: "ESP32",
        title_en: "ESP32",
        intro: "用 ESP-IDF 做商业级产品：从选型、量产到端侧与云端 AI",
        intro_en: "Commercial products with ESP-IDF, from choosing a chip to mass production and AI on the device and in the cloud",
        brief: "只讲乐鑫官方的专业开发路线：ESP-IDF（C 语言，FreeRTOS）、官方工具链（idf.py、VS Code 的 ESP-IDF 插件）和乐鑫官方的组件与框架；不讲 Arduino、MicroPython 和 PlatformIO。读者会 C 语言，目标是做出能量产、能上市的产品。默认开发板是 ESP32-S3（N16R8），语音和视觉需要它的算力与 PSRAM；Matter 与 Thread 用 ESP32-C6 或 H2，屏幕与多媒体用 ESP32-P4。建议的章节路线：芯片与选型（全系列对比、模组与量产型号、BOM 成本）→ ESP-IDF 工程化（安装、工程结构、组件与 CMake、Kconfig 与 menuconfig、组件管理器、JTAG 与 GDB 调试、日志与 core dump，用 AI 编程工具写和调试固件）→ FreeRTOS 与系统（任务与同步、内存与 PSRAM、分区表、NVS、看门狗）→ 外设与驱动（GPIO、LEDC、ADC、I²C、SPI、UART、RMT、I2S 音频、esp_lcd 屏幕与触摸、摄像头）→ 联网（Wi-Fi 与配网：BluFi、SoftAP；HTTPS、WebSocket、MQTT；NimBLE；ESP-NOW；Thread、Zigbee 与 Matter）→ 安全与量产（Secure Boot v2、Flash 加密、eFuse、TLS 证书、OTA 双分区与回滚、产测与烧录、ESP Insights、无线认证）→ 低功耗与产品化（Light Sleep、Deep Sleep、ULP、电源与可靠性）→ 端侧 AI（ESP-SR 的 AFE、唤醒词与命令词，ESP-DL 的模型部署与量化，ESP-WHO 视觉，ESP32-P4 的 AI 与多媒体）→ 接入大模型（HTTPS 调用大模型 API、流式输出、语音链路：识别、大模型、合成，实时语音，小智 AI 等开源项目的架构，用 MCP 让大模型控制设备）→ 实战项目（AI 语音硬件、Matter 智能家居设备、视觉终端）。API、配置项和命令以 ESP-IDF 官方编程指南的最新稳定版为准，写明适用的 ESP-IDF 版本和芯片。",
    },
];

pub(crate) fn track(id: &str) -> Option<&'static Track> {
    TRACKS.iter().find(|track| track.id == id)
}

const LEARN: Chrome<'static> = Chrome {
    current: "learn",
    section: "reads learn",
    stylesheet: "/reads.css?v=18",
    script: "/reads.js?v=9",
    feed: ("News", "/feed.xml"),
};

pub(super) fn router() -> Router<AppState> {
    reading::pages(
        Router::new()
            .route(PATH, get(map))
            .route("/learn/{track}", get(contents)),
    )
    .layer(tower_http::compression::CompressionLayer::new())
}

#[derive(Deserialize)]
struct Params {
    lang: Option<String>,
}

/// A public lesson as a track's contents list it.
#[derive(FromRow)]
pub(super) struct Lesson {
    pub(super) id: Uuid,
    pub(super) chapter: String,
    pub(super) lesson: i32,
    pub(super) title: String,
    pub(super) published_at: DateTime<Utc>,
    pub(super) updated_at: DateTime<Utc>,
}

/// A track's public lessons in order.
pub(super) async fn lessons(state: &AppState, track: &str) -> Result<Vec<Lesson>> {
    Ok(sqlx::query_as(
        "SELECT a.id, a.chapter, a.lesson, a.title, a.published_at, a.updated_at
         FROM articles a JOIN users u ON u.id = a.author_id
         WHERE u.status AND a.status = 'published' AND a.course = $1 ORDER BY a.lesson",
    )
    .bind(track)
    .fetch_all(&state.pool)
    .await?)
}

/// Each lesson's chapter number: a chapter starts where a lesson names another than the lesson
/// before it.
pub(super) fn numbered<'a>(chapters: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut last = None;
    let mut number = 0;
    chapters
        .into_iter()
        .map(|chapter| {
            if last != Some(chapter) {
                number += 1;
                last = Some(chapter);
            }
            number
        })
        .collect()
}

/// The courses by group, in the order they are listed.
fn groups() -> Vec<(Group, Vec<&'static Track>)> {
    let mut groups: Vec<(Group, Vec<&'static Track>)> = Vec::new();
    for track in &TRACKS {
        match groups.last_mut() {
            Some((group, tracks)) if *group == track.group => tracks.push(track),
            _ => groups.push((track.group, vec![track])),
        }
    }
    groups
}

/// A page with its contents beside it: on a wide screen they stand left of the page and stay in
/// view, on a narrow one the page stands alone (reads.css).
pub(super) fn columns(side: &str, main: &str) -> String {
    format!("<div class=learn-layout>{side}<div class=learn-main>{main}</div></div>")
}

/// The way back to every course, at the head of the contents.
fn back(en: bool) -> String {
    format!(
        "<a class=learn-back href=\"{}\">← {}</a>",
        localized(PATH, en),
        choose(en, "全部课程", "All courses")
    )
}

fn course(track: &Track, en: bool) -> String {
    escape(&localized(&format!("{PATH}/{}", track.id), en))
}

/// Beside a course's page: every course by group, this one marked.
fn courses(current: &str, en: bool) -> String {
    let mut html = format!(
        "<nav class=learn-side aria-label=\"{}\">{}",
        choose(en, "全部课程", "All courses"),
        back(en)
    );
    for ((name, name_en), tracks) in groups() {
        let _ = write!(html, "<h2>{}</h2><ul>", choose(en, name, name_en));
        for track in tracks {
            let _ = write!(
                html,
                "<li><a href=\"{}\"{}>{}</a></li>",
                course(track, en),
                if track.id == current {
                    " aria-current=\"page\""
                } else {
                    ""
                },
                choose(en, track.title, track.title_en)
            );
        }
        html.push_str("</ul>");
    }
    html.push_str("</nav>");
    html
}

fn day(at: DateTime<Utc>, state: &AppState) -> String {
    at.with_timezone(&state.config.timezone)
        .format("%m-%d")
        .to_string()
}

/// How many lessons and chapters a course has.
fn size(lessons: usize, chapters: usize, en: bool) -> String {
    choose(
        en,
        &format!("{lessons} 课 · {chapters} 章"),
        &format!("{lessons} lessons · {chapters} chapters"),
    )
    .to_owned()
}

/// `/learn`: the courses by group, how many lessons each has and when one last changed.
async fn map(
    State(state): State<AppState>,
    reader: Reader,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let counts: HashMap<String, (i64, DateTime<Utc>)> = sqlx::query_as::<_, (String, i64, DateTime<Utc>)>(
        "SELECT a.course, count(*), max(a.updated_at) FROM articles a JOIN users u ON u.id = a.author_id
         WHERE u.status AND a.status = 'published' AND a.course IS NOT NULL GROUP BY a.course",
    )
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|(course, count, updated)| (course, (count, updated)))
    .collect();
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p></header><div class=learn-map>",
        label(en),
        choose(
            en,
            "AI 与 ESP32 课程，持续更新，常学常新",
            "Courses on AI and the ESP32, growing and kept current around the clock"
        ),
    );
    for ((name, name_en), tracks) in groups() {
        let _ = write!(
            content,
            "<section class=learn-group><h2>{}</h2>",
            choose(en, name, name_en)
        );
        for track in tracks {
            let meta = match counts.get(track.id) {
                Some((count, updated)) => choose(
                    en,
                    &format!("{count} 课 · 更新 {}", day(*updated, &state)),
                    &format!("{count} lessons · updated {}", day(*updated, &state)),
                )
                .to_owned(),
                None => choose(en, "第一课正在写", "The first lesson is being written").to_owned(),
            };
            let _ = write!(
                content,
                "<div class=learn-track><h3><a href=\"{}\">{}</a><span class=learn-meta> · {meta}</span></h3><p>{}</p></div>",
                course(track, en),
                choose(en, track.title, track.title_en),
                choose(en, track.intro, track.intro_en),
            );
        }
        content.push_str("</section>");
    }
    content.push_str("</div>");
    let html = crate::site::layout(
        &state,
        reader,
        en,
        PATH,
        &format!("{} · 𝑵𝒆𝒘𝒔", label(en)),
        choose(
            en,
            "AI 入门、提示词、AI 编程、Agent 与 MCP、RAG、模型 API、本地模型、AI 创作与 ESP32 的系列课程，持续更新",
            "Courses on AI basics, prompting, AI coding, agents and MCP, RAG, model APIs, local models, creating with AI and the ESP32, kept current",
        ),
        &content,
        None,
        &LEARN,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// Lessons new or revised within this many days lead a track's page, unless they are all it has.
const RECENT_DAYS: i64 = 7;
/// How many of them, at most.
const RECENT: usize = 5;

/// `/learn/{track}`: the track's lessons by chapter, after the latest new or revised ones, with
/// every course beside them.
async fn contents(
    State(state): State<AppState>,
    reader: Reader,
    Path(id): Path<String>,
    Query(params): Query<Params>,
    headers: HeaderMap,
) -> Result<Response> {
    let en = params.lang.as_deref() == Some("en");
    let track = track(&id).ok_or_else(|| AppError::NotFound("课程不存在".into()))?;
    let lessons = lessons(&state, track.id).await?;
    let numbers = numbered(lessons.iter().map(|lesson| lesson.chapter.as_str()));
    let href = |lesson: &Lesson| localized(&format!("/articles/{}", lesson.id), en);
    let name = |lesson: &Lesson| {
        format!(
            "<span class=learn-number>{}</span>{}",
            choose(
                en,
                &format!("第 {} 课", lesson.lesson),
                &format!("Lesson {}", lesson.lesson)
            ),
            escape(&lesson.title)
        )
    };
    let time = |at: DateTime<Utc>| {
        format!(
            "<time datetime=\"{}\">{}</time>",
            at.to_rfc3339(),
            day(at, &state)
        )
    };
    let mut content = format!(
        "<header class=reads-heading><h1>{}</h1><p>{}</p></header>",
        choose(en, track.title, track.title_en),
        choose(en, track.intro, track.intro_en),
    );
    match lessons.iter().map(|lesson| lesson.updated_at).max() {
        Some(updated) => {
            let _ = write!(
                content,
                "<p class=learn-stats>{} · {} {}</p>",
                size(
                    lessons.len(),
                    numbers.last().copied().unwrap_or_default(),
                    en
                ),
                choose(en, "最近更新", "updated"),
                day(updated, &state)
            );
        }
        None => {
            let _ = write!(
                content,
                "<p class=empty>{}</p>",
                choose(en, "第一课正在写。", "The first lesson is being written.")
            );
        }
    }
    let since = Utc::now() - chrono::Duration::days(RECENT_DAYS);
    let mut recent: Vec<&Lesson> = lessons
        .iter()
        .filter(|lesson| lesson.updated_at > since)
        .collect();
    recent.sort_by_key(|lesson| std::cmp::Reverse(lesson.updated_at));
    recent.truncate(RECENT);
    if !recent.is_empty() && recent.len() < lessons.len() {
        let _ = write!(
            content,
            "<section class=learn-recent><h2>{}</h2><ul>",
            choose(en, "最近更新", "Latest")
        );
        for lesson in recent {
            // A lesson revised after it went out says so; a new one says it is new.
            let revised = lesson.updated_at - lesson.published_at > chrono::Duration::hours(1);
            let _ = write!(
                content,
                "<li><span class=learn-badge>{}</span><a href=\"{}\">{}</a>{}</li>",
                if revised {
                    choose(en, "更新", "Updated")
                } else {
                    choose(en, "新", "New")
                },
                escape(&href(lesson)),
                name(lesson),
                time(lesson.updated_at)
            );
        }
        content.push_str("</ul></section>");
    }
    let mut chapter = 0;
    for (lesson, number) in lessons.iter().zip(&numbers) {
        if *number != chapter {
            if chapter > 0 {
                content.push_str("</ol></section>");
            }
            chapter = *number;
            let _ = write!(
                content,
                "<section class=learn-chapter><h2><span class=learn-number>{}</span>{}</h2><ol class=learn-lessons>",
                choose(en, &format!("第 {number} 章"), &format!("Chapter {number}")),
                escape(&lesson.chapter)
            );
        }
        let _ = write!(
            content,
            "<li><a href=\"{}\">{}</a>{}</li>",
            escape(&href(lesson)),
            name(lesson),
            time(lesson.updated_at)
        );
    }
    if chapter > 0 {
        content.push_str("</ol></section>");
    }
    let html = crate::site::layout(
        &state,
        reader,
        en,
        &format!("{PATH}/{}", track.id),
        &format!(
            "{} · {}",
            choose(en, track.title, track.title_en),
            label(en)
        ),
        choose(en, track.intro, track.intro_en),
        &columns(&courses(track.id, en), &content),
        None,
        &LEARN,
    );
    Ok(conditional(
        &headers,
        &Rendered::new(html),
        "text/html; charset=utf-8",
    ))
}

/// Where a lesson stands: its track, its number and chapter, among the track's public lessons.
pub(super) struct Place {
    track: &'static Track,
    lesson: i32,
    chapter: usize,
    name: String,
    lessons: Vec<Lesson>,
    numbers: Vec<usize>,
}

impl Place {
    /// The lesson numbered `lesson` in `course`; a lesson not public, as a preview shows it, still
    /// finds its chapter and neighbours.
    pub(super) async fn find(
        state: &AppState,
        course: &str,
        chapter: &str,
        lesson: i32,
    ) -> Result<Option<Self>> {
        let Some(track) = track(course) else {
            return Ok(None);
        };
        let lessons = lessons(state, course).await?;
        let numbers = numbered(lessons.iter().map(|lesson| lesson.chapter.as_str()));
        // Its chapter continues the one before it when it bears that name, or else follows it.
        let chapter_number = match lessons.iter().rposition(|other| other.lesson < lesson) {
            Some(at) if lessons[at].chapter == chapter => numbers[at],
            Some(at) => numbers[at] + 1,
            None => 1,
        };
        Ok(Some(Self {
            track,
            lesson,
            chapter: chapter_number,
            name: chapter.to_owned(),
            lessons,
            numbers,
        }))
    }

    /// Above the title: 学习 › the track › the chapter.
    pub(super) fn crumbs(&self, en: bool) -> String {
        format!(
            "<p class=learn-crumbs><a href=\"{}\">{}</a> › <a href=\"{}\">{}</a> › {} {}</p>",
            localized(PATH, en),
            label(en),
            course(self.track, en),
            choose(en, self.track.title, self.track.title_en),
            choose(
                en,
                &format!("第 {} 章", self.chapter),
                &format!("Chapter {}", self.chapter)
            ),
            escape(&self.name)
        )
    }

    /// Beside the lesson: its course's chapters, each folding its lessons away but its own, which
    /// stands open with the lesson marked. A lesson not public, as a preview shows it, is not among
    /// them, and opens none.
    pub(super) fn contents(&self, en: bool) -> String {
        let open = self
            .lessons
            .iter()
            .position(|lesson| lesson.lesson == self.lesson)
            .map(|at| self.numbers[at]);
        let mut html = format!(
            "<nav class=learn-side aria-label=\"{}\">{}<a class=learn-course href=\"{}\">{}</a><p class=learn-meta>{}</p>",
            choose(en, "课程目录", "Contents"),
            back(en),
            course(self.track, en),
            choose(en, self.track.title, self.track.title_en),
            size(
                self.lessons.len(),
                self.numbers.last().copied().unwrap_or_default(),
                en
            )
        );
        let mut chapter = 0;
        for (lesson, number) in self.lessons.iter().zip(&self.numbers) {
            if *number != chapter {
                if chapter > 0 {
                    html.push_str("</ol></details>");
                }
                chapter = *number;
                let _ = write!(
                    html,
                    "<details{}><summary><span class=learn-number>{number}</span>{}</summary><ol>",
                    if Some(chapter) == open { " open" } else { "" },
                    escape(&lesson.chapter)
                );
            }
            let _ = write!(
                html,
                "<li><a href=\"{}\"{}><span class=learn-number>{}</span>{}</a></li>",
                escape(&localized(&format!("/articles/{}", lesson.id), en)),
                if lesson.lesson == self.lesson {
                    " aria-current=\"page\""
                } else {
                    ""
                },
                lesson.lesson,
                escape(&lesson.title)
            );
        }
        if chapter > 0 {
            html.push_str("</ol></details>");
        }
        html.push_str("</nav>");
        html
    }

    /// Under the lesson: the lessons before and after it, and the way back to the contents.
    pub(super) fn steps(&self, en: bool) -> String {
        let step = |lesson: Option<&Lesson>, rel: &str, arrow: (&str, &str)| {
            lesson
                .map(|lesson| {
                    format!(
                        "<a rel={rel} href=\"{}\">{}{}{}</a>",
                        escape(&localized(&format!("/articles/{}", lesson.id), en)),
                        arrow.0,
                        escape(&format!(
                            "{} {}",
                            choose(
                                en,
                                &format!("第 {} 课", lesson.lesson),
                                &format!("Lesson {}", lesson.lesson)
                            ),
                            lesson.title
                        )),
                        arrow.1
                    )
                })
                .unwrap_or_default()
        };
        let previous = self
            .lessons
            .iter()
            .rev()
            .find(|other| other.lesson < self.lesson);
        let next = self.lessons.iter().find(|other| other.lesson > self.lesson);
        format!(
            "<nav class=learn-steps aria-label=\"{}\">{}{}</nav><div class=read-actions><a href=\"{}\">← {}</a></div>",
            choose(en, "上一课与下一课", "Previous and next lessons"),
            if previous.is_some() {
                step(previous, "prev", ("← ", ""))
            } else {
                "<span></span>".into()
            },
            step(next, "next", ("", " →")),
            course(self.track, en),
            choose(
                en,
                &format!("{}目录", self.track.title),
                &format!("{} contents", self.track.title_en)
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_are_numbered_where_their_names_change() {
        assert_eq!(
            numbered(["选型", "选型", "工程化", "工程化", "工程化", "外设"]),
            [1, 1, 2, 2, 2, 3]
        );
        // A name coming back later starts a chapter again, so the numbers keep the order.
        assert_eq!(numbered(["甲", "乙", "甲"]), [1, 2, 3]);
        assert!(numbered([]).is_empty());
    }

    #[test]
    fn tracks_have_distinct_addresses_and_stand_together_by_group() {
        let mut ids: Vec<&str> = TRACKS.iter().map(|track| track.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), TRACKS.len());
        for track in &TRACKS {
            assert!(
                track
                    .id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            );
            assert!(!track.brief.is_empty() && !track.intro.is_empty() && !track.title.is_empty());
        }
        // A group listed twice would split its courses.
        let groups: Vec<Group> = groups().into_iter().map(|(group, _)| group).collect();
        let mut distinct = groups.clone();
        distinct.dedup();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(groups.len(), distinct.len());
        assert_eq!(groups, [STARTING, BUILDING, HARDWARE]);
    }
}
