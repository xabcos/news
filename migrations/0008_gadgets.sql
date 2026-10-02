-- Culture's catalogue also serves 𝑮𝒂𝒅𝒈𝒆𝒕𝒔, so its tables take neutral names.
ALTER TABLE culture_works RENAME TO catalog_entries;
ALTER TABLE culture_items RENAME TO catalog_items;
ALTER TABLE catalog_items RENAME COLUMN work_id TO entry_id;
ALTER INDEX culture_items_work_idx RENAME TO catalog_items_entry_idx;
ALTER TABLE catalog_entries DROP CONSTRAINT culture_works_topic_id_check;

-- An entry is known by its stable page in Culture and by brand and model in 𝑮𝒂𝒅𝒈𝒆𝒕𝒔,
-- whose products are cited through several official pages over time.
ALTER TABLE catalog_entries RENAME COLUMN source_url TO key;
-- Official pictures of a product, read from its official sources.
ALTER TABLE catalog_entries ADD COLUMN images jsonb NOT NULL DEFAULT '[]';

-- Reviews and shares stay with their product after news retention removes headlines.
ALTER TABLE catalog_items ADD COLUMN role text NOT NULL DEFAULT 'news'
    CHECK (role IN ('news', 'review', 'share'));

DROP INDEX items_culture_order_idx;
CREATE INDEX items_catalog_order_idx ON items (topic_id, first_seen_at DESC, id DESC)
    WHERE NOT hidden AND topic_id IN ('movie', 'music', 'game', 'book', 'eink', 'ai-hardware', 'digital');

-- Three collections under one 𝑮𝒂𝒅𝒈𝒆𝒕𝒔 entry. Official sources are the domains and X accounts
-- named in each brief; the program accepts product pages only from them.
INSERT INTO topics (id, name, name_en, description, brief, enabled, visible,
    interval_minutes, window_start, window_end, weekdays, model, reasoning_effort,
    max_items, lookback_hours, daily_run_cap, sort_weight, created_at)
SELECT id, name, name, description, brief, true, true, minutes, 0, 24,
    '{1,2,3,4,5,6,7}'::smallint[], 'grok-4.7', 'medium', items, 48, runs, 80,
    NOW() + position * INTERVAL '1 millisecond'
FROM (VALUES
    (1, 'eink', '𝑬𝒊𝒏𝒌', '以 Kindle 为首的电纸书与墨水屏设备：发布、真实测评与用户分享',
     '全球与国内的电纸书和墨水屏设备，以 Kindle 为首：阅读器、手写本、墨水屏手机与显示器、E Ink 屏幕技术，以及固件、系统和书店服务的变化。重点是真实上手测评和用户在 X 上的分享，兼顾官方发布；传闻注明。官方来源：amazon.com aboutamazon.com kobo.com boox.com pocketbook-int.com remarkable.com supernote.com ireader.com hanvon.com eink.com mi.com huawei.com bigme.vip；官方账号：@AmazonKindle @kobo @remarkablepaper。',
     360, 12, 4),
    (2, 'ai-hardware', '𝑨𝑰 𝑯𝒂𝒓𝒅𝒘𝒂𝒓𝒆', 'AI 眼镜、AI 录音与助手设备等 AI 硬件：发布、真实测评与用户分享',
     '以 AI 为核心的新形态硬件：AI 眼镜、AI 录音卡片与挂件、AI 助手设备、陪伴与桌面机器人、端侧 AI 主机。手机和电脑归数码，不在这里。重点是真实上手测评和用户在 X 上的分享，兼顾官方发布；传闻注明。官方来源：meta.com ray-ban.com xreal.com rokid.com rayneo.com evenrealities.com brilliant.xyz plaud.ai limitless.ai rabbit.tech openai.com store.google.com blog.google apple.com samsung.com mi.com huawei.com nvidia.com；官方账号：@Meta @OpenAI @nvidia。',
     480, 10, 3),
    (3, 'digital', '𝑫𝒊𝒈𝒊𝒕𝒂𝒍', '手机、平板、电脑、音频、穿戴、影像、游戏与家居：发布、真实测评与用户分享',
     '手机、平板、电脑、耳机音频、手表穿戴、相机影像、游戏设备与智能家居的新品和重要更新。电纸书与墨水屏设备归 𝑬𝒊𝒏𝒌，AI 眼镜、录音卡片等 AI 硬件归 𝑨𝑰 𝑯𝒂𝒓𝒅𝒘𝒂𝒓𝒆，不在这里。重点是真实上手测评和用户在 X 上的分享，兼顾官方发布；传闻注明。官方来源：apple.com samsung.com mi.com xiaomi.com huawei.com honor.com oppo.com vivo.com oneplus.com store.google.com blog.google sony.com nothing.tech lenovo.com microsoft.com dell.com asus.com hp.com dji.com insta360.com gopro.com canon.com nikon.com fujifilm.com bose.com sennheiser.com garmin.com nintendo.com playstation.com xbox.com steampowered.com；官方账号：@Apple @Samsung @Xiaomi @Google @nothing。',
     240, 16, 6)
) AS seed(position, id, name, description, brief, minutes, items, runs)
ON CONFLICT (id) DO NOTHING;
