-- Articles readers write in the workspace (分享). An article is public once an administrator gives
-- it a ❤️, and an author's edit takes it back to review. Authors write under a pen name; their
-- email address is never shown.
CREATE TABLE authors (
    user_id bigint PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    name text NOT NULL
        CONSTRAINT authors_name_valid CHECK (char_length(name) BETWEEN 2 AND 24 AND name = btrim(name)),
    -- What the author started on `day`, the site's date: new articles and uploaded pictures.
    -- Deleting an article gives neither back.
    day date,
    articles smallint NOT NULL DEFAULT 0,
    images smallint NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX authors_name_idx ON authors (lower(name));

CREATE TABLE articles (
    id uuid PRIMARY KEY,
    author_id bigint NOT NULL REFERENCES authors(user_id) ON DELETE CASCADE,
    title text NOT NULL DEFAULT '' CONSTRAINT articles_title_valid CHECK (char_length(title) <= 100),
    -- Markdown; a picture is written `![…](image:<id>)` with the id of one of its images.
    body text NOT NULL DEFAULT '' CONSTRAINT articles_body_valid CHECK (char_length(body) <= 50000),
    -- The start of the body as plain text, for lists.
    excerpt text NOT NULL DEFAULT '',
    cover uuid,
    status text NOT NULL DEFAULT 'draft'
        CONSTRAINT articles_status_valid CHECK (status IN ('draft', 'pending', 'published')),
    -- Counts the saves; a ❤️ is given for the revision the administrator read.
    revision integer NOT NULL DEFAULT 1,
    views bigint NOT NULL DEFAULT 0 CONSTRAINT articles_views_valid CHECK (views >= 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now(),
    -- When it first went public; later edits and reviews keep its place in the list.
    published_at timestamptz,
    CONSTRAINT articles_published_valid CHECK (status <> 'published' OR published_at IS NOT NULL)
);
CREATE INDEX articles_public_idx ON articles (published_at DESC, id DESC) WHERE status = 'published';
CREATE INDEX articles_author_idx ON articles (author_id, updated_at DESC);
CREATE INDEX articles_pending_idx ON articles (updated_at) WHERE status = 'pending';

-- tmpfile keeps the pictures. Its links are given out only on public pages, so a picture stays
-- private until its article is.
CREATE TABLE article_images (
    id uuid PRIMARY KEY,
    article_id uuid NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    url text NOT NULL,
    content_type text NOT NULL,
    width integer NOT NULL CHECK (width > 0),
    height integer NOT NULL CHECK (height > 0),
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    CONSTRAINT article_images_article_unique UNIQUE (article_id, id)
);
-- A cover is one of the article's own images.
ALTER TABLE articles ADD CONSTRAINT articles_cover_fkey
    FOREIGN KEY (id, cover) REFERENCES article_images (article_id, id);
