-- The Chinese pages name every public topic in Chinese; the English pages keep the styled names.
UPDATE topics AS t SET name = n.name, updated_at = now()
FROM (VALUES
    ('x', '要闻'), ('p', '图说'), ('w', '佳人'), ('china-ai', '国内 AI'), ('overseas-ai', '海外 AI'),
    ('gossip', '八卦'), ('github', '开源'), ('skills', '技巧'), ('events', '时事'), ('cases', '案例'),
    ('issues', '踩坑'), ('movie', '影视'), ('music', '音乐'), ('game', '游戏'), ('book', '图书'),
    ('eink', '电纸书'), ('ai-hardware', 'AI 硬件'), ('digital', '数码'), ('travel', '旅行'),
    ('fashion', '时尚'), ('reads', '精读'), ('llm', '大模型'), ('equity', '股市资讯')
) AS n (id, name)
WHERE t.id = n.id AND t.owner_user_id IS NULL;
