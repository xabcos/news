-- Rewrite legacy owner-facing messages after the inbox was removed.
UPDATE cat_messages
SET payload = jsonb_set(payload, '{text}', to_jsonb('我记住了我们第一次相遇。接下来，我会自己生活、拍下新的动态照片，也会去图片新闻里的地点旅行。'::text))
WHERE kind = 'welcome';

UPDATE cat_messages
SET payload = jsonb_set(payload, '{text}', to_jsonb('我出发啦，回来给你带照片。'::text))
WHERE kind = 'travel' AND dedupe_key LIKE '%:departed';

UPDATE cat_messages
SET payload = jsonb_set(
        jsonb_set(payload, '{text}', to_jsonb('我回到猫窝了，把旅途照片带回来给你。'::text)),
        '{title}', to_jsonb('旅程照片'::text)
    )
WHERE kind = 'postcard';

UPDATE cat_messages
SET payload = jsonb_set(payload, '{text}', to_jsonb('我把新的旅行配饰收进自己的旅途了。'::text))
WHERE kind = 'growth';
