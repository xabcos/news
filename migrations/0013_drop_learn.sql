-- 学习 has its own site now, learn.xabcnews.com, which took its courses, lessons and pictures
-- along. The lessons leave with their pictures (which cascade), and so do the courses, the columns
-- that placed a lesson in its course (their constraint, index and key go with them), and the
-- weekly look for a new course.
DELETE FROM articles WHERE course IS NOT NULL;
ALTER TABLE articles
    DROP COLUMN course,
    DROP COLUMN chapter,
    DROP COLUMN lesson,
    DROP COLUMN keywords,
    DROP COLUMN review_at;
DROP TABLE courses;
DELETE FROM maintenance_state WHERE task = 'course_discovery';
