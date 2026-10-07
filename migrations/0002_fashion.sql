-- 𝑭𝒂𝒔𝒉𝒊𝒐𝒏 starts as an administrator's preview; the workbench puts it in front of readers.
INSERT INTO topics (
    id, name, name_en, description, brief, enabled, visible, interval_minutes, window_start,
    window_start_minute, window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
    daily_run_cap, sort_weight
) VALUES
    ('fashion', '𝑭𝒂𝒔𝒉𝒊𝒐𝒏', '𝑭𝒂𝒔𝒉𝒊𝒐𝒏', '以 Vogue 为主的秀场、明星穿搭、潮流单品与美妆',
$brief$中文：面向关注时尚的女性读者，以 Vogue（vogue.com）为主要来源，兼收 Vogue 各国版（含 Vogue 中国 vogue.com.cn）、Harper's BAZAAR、ELLE、WWD、Business of Fashion 和品牌官方账号。收录四类：秀场与设计师（纽约、伦敦、米兰、巴黎四大时装周与上海、东京、首尔、哥本哈根等时装周，高级定制、度假与早秋系列，后台与街拍，创意总监任命、首秀与重要合作）；明星穿搭与红毯（明星的日常穿搭、活动与红毯造型，兼顾中国与亚洲明星）；潮流趋势与单品（本季流行、关键单品与穿法）；美妆（彩妆、发型、美甲、护肤与秀场妆容）。秀场每场一条，优先重要品牌和受关注的新设计师。不收购物折扣、带货清单与赞助内容，也不收婚礼、王室、派对、纯商业财报和影视书籍新闻。
English: For women who follow fashion, use Vogue (vogue.com) as the main source, plus Vogue's international editions (including Vogue China, vogue.com.cn), Harper's BAZAAR, ELLE, WWD, Business of Fashion and brands' official accounts. Cover four kinds: runway and designers (the New York, London, Milan and Paris fashion weeks and those of Shanghai, Tokyo, Seoul, Copenhagen and others; couture, resort and pre-fall collections; backstage and street style; creative-director appointments, debuts and major collaborations); celebrity style and red carpets (stars' everyday style, events and red-carpet looks, Chinese and Asian stars included); trends and key pieces (what is in this season, the key pieces and how to wear them); and beauty (makeup, hair, nails, skincare and runway beauty). One item per show, favoring major houses and designers people are watching. Skip shopping deals, affiliate lists and sponsored content, as well as weddings, royals, parties, pure business results, and film or book news.$brief$,
     true, false, 128, 0, 0, 24, '{1,2,3,4,5,6,7}', 'grok-4.7', 'high', 30, 24, 24, 81)
ON CONFLICT (id) DO NOTHING;
