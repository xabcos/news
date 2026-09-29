-- Equity notes are read by cutoff: the latest note, a chosen edition and the review
-- baseline all order by window_end, which the briefing_day index cannot serve, so every
-- read would otherwise decompress each stored payload before sorting.
CREATE INDEX ai_insights_kind_window_idx
    ON ai_insights (kind, window_end DESC, finished_at DESC, id DESC) WHERE status = 'ok';
