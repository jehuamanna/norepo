-- Project-tier Claude defaults. Resolution at spawn time is
-- chat → project → global → omit flag. Each NULL means "inherit
-- from the next layer down."
--
-- Without these columns there was no way to set a default model or
-- permission mode that applies to every chat in a project — users
-- had to repeat the picker change in every new chat. The chat-tier
-- columns (migration 017) and the global setting in
-- `local_app_settings` together with these columns form the full
-- three-tier hierarchy.

ALTER TABLE local_project ADD COLUMN default_model TEXT NULL;
ALTER TABLE local_project ADD COLUMN default_permission_mode TEXT NULL;

INSERT INTO _schema_migrations (version, applied_at_ms) VALUES (19, 0);
