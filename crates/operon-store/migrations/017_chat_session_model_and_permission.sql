-- Per-chat Claude model + permission_mode persistence.
--
-- Until now the companion-pane pickers updated the
-- ClaudeCodeChatPlugin's global state in memory only, so switching
-- between chats or restarting the app reverted every chat to the
-- plugin defaults. These two columns store the user's per-chat
-- choice; the UI writes through here on every picker change and
-- reads back when a chat is bound (chat-switch or app start) so the
-- preference survives both.
--
-- Both columns are nullable: NULL means "fall back to the global
-- default" (which is what `claude --model …` / `claude
-- --permission-mode …` omitting the flag does today).

ALTER TABLE chat_session ADD COLUMN model TEXT NULL;
ALTER TABLE chat_session ADD COLUMN permission_mode TEXT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (17, 0);
