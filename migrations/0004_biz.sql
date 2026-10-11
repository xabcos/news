-- 商机 (𝑩𝒊𝒛) collects business intelligence from X. It starts as an administrator's preview;
-- the workbench puts it in front of readers.
INSERT INTO topics (
    id, name, name_en, description, brief, enabled, visible, interval_minutes, window_start,
    window_start_minute, window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
    daily_run_cap, sort_weight
) VALUES
    ('biz', '商机', '𝑩𝒊𝒛', 'X 上的一手商业信息：新机会、实战复盘、需求痛点、出海电商与获客增长',
$brief$中文：在 X 上收集对做生意有用的一手信息，每条都要说清钱在哪里、怎么赚或机会和风险在哪。五类：新机会（已出现付费需求或收入证据的新生意、新产品形态、新平台与新渠道，以及平台规则、费用、关税、支付和政策变化对生意的具体影响）；实战复盘（当事人公开的收入、订单、利润、转化、获客成本、广告回报等数字，以及可借鉴的做法、渠道、定价或教训；只晒数字、没有可借鉴做法的小额个人进度不收）；需求痛点（很多人愿意付费却没被满足的需求、细分市场空白，要有大量讨论、求推荐或付费意愿的证据）；出海电商（跨境电商、独立站、亚马逊、TikTok Shop、App 与 SaaS 出海的选品、流量、定价与平台规则）；获客增长（找客户、冷启动、广告、SEO、内容与渠道的打法，带数字和结果）。中文与英文大致各半：中文用出单、月入、复盘、新词、独立站、选品、出海、获客、副业、求推荐等词，点赞下限约 10；英文用 MRR、ARR、revenue、first customers、ROAS、cold email、I'd pay for 等词，点赞下限约 20；都按热度排序，结果太少时放低。优先当事人一手分享和有具体数字、可核实细节、讨论多的帖子，条目链接用 X 帖子地址，必要时用网页检索核对产品或公司；作者自述的数字写明是自称。tags 与 tags_en 的第一个写类别：新机会、实战复盘、需求痛点、出海电商或获客增长（Opportunity、Results、Demand、Global Commerce、Growth）。不收：AI 与编程工具的一般用法和技巧、模型发布与定价、招聘就业观点和宏观评论（这些归其他栏目）；带推广链接或邀请码的软文、卖课和付费社群、加密货币喊单、刷单养号等灰产、收集他人联系方式群发、没有数据的暴富宣传、搬运和政治。
English: Collect first-hand business intelligence from X; every item must say where the money is, how it is made, or what opportunity or risk it creates. Kinds: new opportunities with evidence of paying demand or revenue, and concrete effects of platform, fee, tariff, payment or policy changes; first-hand results with revenue, orders, profit, conversion, acquisition cost or ad return, plus transferable methods, channels, pricing or lessons, not bare progress updates; unmet demand with evidence of willingness to pay; cross-border e-commerce and apps or SaaS going global; growth and customer acquisition playbooks with numbers. Search Chinese and English about equally, by popularity with about 10 (Chinese) or 20 (English) minimum likes; link the X post; mark self-reported numbers as claims. The first tag is the category. Exclude AI or coding tool tips, model launches and pricing, hiring opinions, macro commentary, affiliate promotion, courses and paid communities, crypto shilling, gray-market tactics, contact harvesting, get-rich claims, reposts and politics.
关注账号（检索线索，使用时核实来源）：@levelsio @marc_louvion @dannypostmaa @tdinh_me @arvidkahl @thepatwalls @gregisenberg @0xROAS @gefei55 @idoubicc @luoleiorg$brief$,
     true, false, 256, 0, 0, 24, '{1,2,3,4,5,6,7}', 'grok-4.7', 'high', 24, 24, 24, 75)
ON CONFLICT (id) DO NOTHING;
