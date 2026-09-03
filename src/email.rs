use crate::{error::Result, state::AppState};
use chrono::Utc;
use serde_json::json;

const STREAM_SCHEMA: &str = "v1";

pub(crate) async fn send_login_email(state: &AppState, to: &str, login_url: &str) -> Result<bool> {
    let ttl = login_ttl_label(state.config.email_login_ttl.as_secs());
    let html = login_email_html(to, login_url, &ttl);
    enqueue_email(state, to, "登录新闻服务", html).await
}

async fn enqueue_email(state: &AppState, to: &str, subject: &str, html: String) -> Result<bool> {
    let Some(mut redis) = state.redis.clone() else {
        return Ok(false);
    };
    let payload = json!({
        "oid": state.config.email_oid,
        "to": [to],
        "subject": subject,
        "html": html,
        "reply_to": state.config.email_reply_to,
    });
    let _: String = redis::cmd("XADD")
        .arg(stream_key(&state.config.email_stream_priority))
        .arg("*")
        .arg("payload")
        .arg(serde_json::to_string(&payload)?)
        .arg("enqueued_at")
        .arg(Utc::now().timestamp_millis().to_string())
        .arg("schema")
        .arg(STREAM_SCHEMA)
        .query_async(&mut redis)
        .await?;
    Ok(true)
}

fn stream_key(priority: &str) -> &'static str {
    match priority {
        "critical" => "email:stream:critical",
        "default" => "email:stream:default",
        _ => "email:stream:important",
    }
}

fn login_ttl_label(seconds: u64) -> String {
    if seconds.is_multiple_of(60 * 60) {
        format!("{} 小时", seconds / (60 * 60))
    } else {
        format!("{} 分钟", seconds / 60)
    }
}

fn login_email_html(to: &str, login_url: &str, ttl: &str) -> String {
    let email = escape_html(to);
    let url = escape_html(login_url);
    format!(
        r#"<!doctype html><html lang="zh-CN"><body style="margin:0;padding:24px;background:#f4f7f5;font-family:system-ui,-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;line-height:1.7;color:#13231f">
<div style="max-width:560px;margin:0 auto;padding:28px;border:1px solid #d7e3df;border-radius:16px;background:#fff">
<div style="margin:0 0 8px;color:#126b56;font-size:12px;font-weight:700;letter-spacing:.08em">一次性登录</div>
<h2 style="margin:0 0 16px;font-size:24px">登录新闻服务</h2>
<p style="margin:0 0 10px">邮箱：<strong>{email}</strong></p>
<p style="margin:0">点击下面的按钮即可直接登录。首次登录会自动创建账户。</p>
<p style="margin:24px 0"><a href="{url}" style="display:inline-block;padding:11px 18px;border-radius:10px;background:#126b56;color:#fff;font-weight:700;text-decoration:none">打开新闻服务</a></p>
<p style="margin:0 0 8px;font-size:13px;color:#64736e">如果按钮无法点击，请复制下面的认证链接并粘贴到浏览器地址栏：</p>
<div style="margin:0 0 18px;padding:12px 14px;border:1px solid #d7e3df;border-radius:10px;background:#f4f7f5;color:#13231f;font-family:ui-monospace,SFMono-Regular,Consolas,monospace;font-size:12px;line-height:1.6;word-break:break-all">{url}</div>
<p style="margin:0;font-size:13px;color:#64736e">链接将在 {ttl}后失效，且只能使用一次。如果不是你本人操作，请忽略这封邮件。</p>
<p style="margin:8px 0 0;font-size:13px;color:#64736e">此链接可以直接登录你的账户，请勿转发或分享。</p>
</div></body></html>"#
    )
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::{escape_html, login_email_html, login_ttl_label};

    #[test]
    fn escapes_markup() {
        assert_eq!(escape_html("<a>&\""), "&lt;a&gt;&amp;&quot;");
    }

    #[test]
    fn formats_login_link_lifetime() {
        assert_eq!(login_ttl_label(30 * 60), "30 分钟");
        assert_eq!(login_ttl_label(2 * 60 * 60), "2 小时");
    }

    #[test]
    fn includes_the_same_escaped_login_url_as_button_and_copyable_text() {
        let html = login_email_html(
            "member@example.com",
            "https://news.example.com/#login_token=a&b",
            "30 分钟",
        );

        assert_eq!(
            html.matches("https://news.example.com/#login_token=a&amp;b")
                .count(),
            2
        );
        assert!(html.contains("复制下面的认证链接"));
        assert!(html.contains("member@example.com"));
        assert!(html.contains("请勿转发或分享"));
        assert!(!html.contains("login_token=a&b"));
    }
}
