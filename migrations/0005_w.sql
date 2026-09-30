-- 𝑾 collects beautiful photographs of women that circulate on X, real or AI-generated. It
-- follows 𝑷 in the navigation. Each post is read back from X's embed data before it is kept,
-- and an item leaves once the file server's copy of its photograph has expired.
INSERT INTO topics (
    id, name, name_en, description, brief, enabled, visible, interval_minutes, window_start,
    window_start_minute, window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
    daily_run_cap, sort_weight
) VALUES (
    'w', '𝑾', '𝑾', 'X 上正在传播的美丽女性照片，真人摄影与 AI 美图',
$brief$中文：从 X 上挑选正在传播的美丽女性照片，真人摄影与 AI 生成图都可以，以欣赏艺术的眼光选择人物、造型、光线与构图俱佳的画面。覆盖人像摄影、时尚大片、街拍、礼服婚纱、传统服饰、泳装比基尼、运动舞蹈、Cosplay 与 AI 人像。排除严重裸露、明确色情和 X 标记为敏感的内容，以及看起来可能未成年的人物、偷拍私照和用真实人物合成的 AI 图。
English: Pick beautiful photographs of women circulating on X, real or AI-generated, chosen with an artist's eye for the person, styling, light and composition. Cover portraits, fashion editorials, street style, gowns and bridal wear, traditional dress, swimwear and bikinis, sport and dance, cosplay and AI portraits. Exclude severe nudity, explicit content and posts X marks as sensitive, anyone who may be a minor, voyeuristic or private photos, and AI images of real, identifiable people.$brief$,
    true, true, 360, 0, 0, 24, '{1,2,3,4,5,6,7}', 'grok-4.7', 'medium', 8, 24, 24, 15
)
ON CONFLICT (id) DO NOTHING;
