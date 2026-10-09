-- 𝑳𝑳𝑴 lists the image, video, audio, embedding and rerank models of its providers beside
-- their language models, each priced in its own unit, and links each provider's API
-- documentation. The catalogue starts over: the next run reads every provider afresh, as
-- an import.
DELETE FROM llm_changes;
DELETE FROM llm_models;
UPDATE llm_sources SET content = NULL, content_hash = NULL, changed_at = NULL;

-- What a model is for: text (language), image, video, audio, or embedding and rerank.
ALTER TABLE llm_models ADD COLUMN category text NOT NULL
    CONSTRAINT llm_models_category_valid
        CHECK (category IN ('text', 'image', 'video', 'audio', 'embedding'));

-- Where the provider's API documentation begins, as its own pages link it.
ALTER TABLE llm_vendors ADD COLUMN docs text
    CONSTRAINT llm_vendors_docs_valid CHECK (docs ~ '^https://' AND char_length(docs) <= 500);

-- xAI and Mistral price every model, media ones included, on a pricing page of their docs;
-- the models pages registered as their price lists name the models only. Mistral, waiting
-- for a readable price list, is tried again by the next run.
UPDATE llm_sources SET kind = 'models', currency = NULL
    WHERE (vendor_id, url) IN (('xai', 'https://docs.x.ai/developers/models'),
                               ('mistral', 'https://docs.mistral.ai/models'));
INSERT INTO llm_sources (vendor_id, kind, url, currency, sort_weight)
    SELECT vendor_id, 'pricing', url, 'USD', 5
    FROM (VALUES ('xai', 'https://docs.x.ai/developers/pricing'),
                 ('mistral', 'https://docs.mistral.ai/inference/pricing')) AS page (vendor_id, url)
    WHERE EXISTS (SELECT 1 FROM llm_vendors WHERE id = page.vendor_id)
    ON CONFLICT (vendor_id, url) DO NOTHING;
UPDATE llm_vendors SET checked_at = NULL WHERE id = 'mistral' AND state <> 'active';

-- A provider's note tells first-party models from hosted ones; it no longer narrows the
-- catalogue to language models.
UPDATE llm_vendors SET note = new.note
    FROM (VALUES
        ('google', '只收录 Gemini API 中 Google 自研的模型（Gemini、Imagen、Veo 等）。'),
        ('xai', '只收录 xAI 自研的模型（Grok、Grok Imagine 与语音模型）。'),
        ('qwen', '只收录阿里云百炼中阿里自研的模型（通义千问 Qwen、通义万相 Wan 等），不收录其托管的第三方模型；价格取中国内地部署的标准价格。'),
        ('zhipu', '只收录智谱自研的模型（GLM、CogView、CogVideoX 等）。'),
        ('minimax', '只收录 MiniMax 自研的文本、语音、视频与图像模型。'),
        ('baidu', '只收录千帆平台中百度自研的模型（文心 ERNIE 等），不收录其托管的第三方模型。'),
        ('stepfun', '只收录阶跃星辰自研的 Step 系列模型。'),
        ('cohere', '只收录 Cohere 自研模型（Command、Embed、Rerank 等）的官方 API，不含 North 应用平台。'),
        ('xiaomi-mimo', '只收录小米 MiMo 自研模型的 API。')
    ) AS new (id, note)
    WHERE llm_vendors.id = new.id;

-- The brief a fresh database starts with follows the wider scope; a brief an administrator
-- has written stays theirs.
UPDATE topics SET brief = $brief$中文：收录各服务商通过 API 按量计费提供的主流模型：大语言模型（对话、推理、编程与多模态理解），以及图像生成、视频生成、语音（合成、识别与实时对话）、嵌入与重排模型。价格、规格和链接只取自服务商自己的官方文档（定价页、模型页、更新日志）；同一模型的别名和日期快照合并为一条。不收录第三方托管的模型，也不收录审核、OCR 模型，以及已下线或仅限特定客户申请的模型。
English: Track mainstream pay-as-you-go API models from each provider: language models (chat, reasoning, coding and multimodal understanding), plus image generation, video generation, audio (speech synthesis, recognition and realtime voice), embedding and rerank models. Prices, specs and links come only from the provider's own documentation; merge aliases and dated snapshots. Exclude third-party hosting, moderation, OCR, retired and invitation-only models.$brief$
WHERE id = 'llm' AND brief = $brief$中文：收录各服务商通过 API 提供的主流大语言模型，包括对话、推理、编程与多模态理解模型。价格、规格和链接只取自服务商自己的官方文档（定价页、模型页、更新日志）；同一模型的别名和日期快照合并为一条。不收录第三方托管的模型，也不收录图像与视频生成、语音、嵌入、重排、审核模型，以及已下线或仅限特定客户申请的模型。
English: Track mainstream API language models from each provider: chat, reasoning, coding and multimodal understanding. Prices, specs and links come only from the provider's own documentation; merge aliases and dated snapshots. Exclude third-party hosting, image or video generation, speech, embedding, reranking, moderation, retired and invitation-only models.$brief$;
