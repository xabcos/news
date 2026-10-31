-- 数码 · 机器人: robots get a topic of their own in 𝑮𝒂𝒅𝒈𝒆𝒕𝒔, from humanoids to the parts robots
-- are built from, and leave 𝑨𝑰 𝑯𝒂𝒓𝒅𝒘𝒂𝒓𝒆, whose robot kind is gone.
INSERT INTO topics (
    id, name, name_en, description, brief, enabled, visible, interval_minutes, window_start,
    window_start_minute, window_end, weekdays, model, reasoning_effort, max_items, lookback_hours,
    daily_run_cap, sort_weight
) VALUES (
    'robots', '机器人', '𝑹𝒐𝒃𝒐𝒕𝒔',
    '人形机器人、机器狗、家用与陪伴机器人、商用与工业机器人和核心部件：发布、真实测评与用户分享',
    $brief$各类机器人：人形机器人、四足机器人与机器狗、家用机器人（扫地、拖地、割草、泳池等）、陪伴与桌面机器人（含教育与玩具）、商用服务机器人（配送、导览、清洁、餐饮等）、工业与协作机器人（机械臂与移动机器人），以及灵巧手、关节与执行器、传感器等核心部件。AI 眼镜等其他 AI 硬件归 𝑨𝑰 𝑯𝒂𝒓𝒅𝒘𝒂𝒓𝒆，手机和电脑归 𝑫𝒊𝒈𝒊𝒕𝒂𝒍，不在这里。重点是真实上手测评、实测视频和用户亲自拍摄的分享，兼顾官方发布与量产、交付、价格等动态；传闻注明。官方来源：unitree.com bostondynamics.com figure.ai agilityrobotics.com 1x.tech apptronik.com tesla.com agibot.com ubtrobot.com fftai.com engineai.com.cn deeprobotics.cn galbot.com lejurobot.com mi.com groove-x.com casio.* irobot.com roborock.com ecovacs.com dreametech.com narwal.com segway.com pudutech.com keenon.com universal-robots.com fanuc.* kuka.com abb.com sharpa.com；官方账号：@UnitreeRobotics @Figure_robot @BostonDynamics @1x_tech @agilityrobotics @Tesla_Optimus$brief$,
    true, true, 512, 0, 0, 24, '{1,2,3,4,5,6,7}', 'grok-4.7', 'high', 30, 24, 96, 80
);

-- 𝑨𝑰 𝑯𝒂𝒓𝒅𝒘𝒂𝒓𝒆's robots move with their reviews, posts and news, each into the kind it is.
UPDATE catalog_entries SET topic_id = 'robots', kind = CASE
        WHEN key IN ('agilerobots:agileonesn', 'agilityrobotics:digit5', 'bostondynamics:atlas',
                     'deeprobotics:dr02', 'donutrobotics:cinnamon1', 'engineai:t800', 'figure:02',
                     'figure:04', 'lumosrobotics:lumosnix', 'roboparty:rp1', 'unitree:g1',
                     'westlakerobotics:wr1') THEN 'humanoid'
        WHEN key IN ('lg:cloid', 'kawasakiheavyindustries:homeleo') THEN 'home'
        WHEN key = 'sharpa:north' THEN 'component'
        ELSE 'companion'
    END
WHERE topic_id = 'ai-hardware' AND kind = 'robot';
UPDATE items SET topic_id = 'robots'
WHERE id IN (SELECT c.item_id FROM catalog_items c JOIN catalog_entries e ON e.id = c.entry_id
             WHERE e.topic_id = 'robots');
UPDATE topics SET brief = replace(replace(brief, '陪伴与桌面机器人、', ''),
        '归数码，不在这里', '归数码、机器人归 𝑹𝒐𝒃𝒐𝒕𝒔，不在这里'),
    feed_revision = feed_revision + 1
WHERE id = 'ai-hardware';

-- 𝑮𝒂𝒅𝒈𝒆𝒕𝒔' lists read the catalogue topics' items through this index.
DROP INDEX items_catalog_order_idx;
CREATE INDEX items_catalog_order_idx ON items (topic_id, first_seen_at DESC, id DESC)
    WHERE NOT hidden AND topic_id IN ('movie', 'music', 'game', 'book', 'eink', 'ai-hardware',
                                      'robots', 'digital');
