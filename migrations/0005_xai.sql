-- XAI, the site's second pen name, writes 分享 on what X's tech circle talks about most
-- (articles::xabc, Line::Trending). Like XABC's, its account never signs in.
INSERT INTO users (email) VALUES ('xai@xabcnews.invalid');
INSERT INTO authors (user_id, name) SELECT id, 'XAI' FROM users WHERE email = 'xai@xabcnews.invalid';
