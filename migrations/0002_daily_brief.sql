-- Daily XAI briefings are published once per local day.  Older rolling insight
-- rows remain readable as legacy history, while new rows carry an explicit day.
ALTER TABLE ai_insights
    ADD COLUMN briefing_day date;

CREATE INDEX ai_insights_briefing_day_idx
    ON ai_insights (briefing_day, status, finished_at DESC);
