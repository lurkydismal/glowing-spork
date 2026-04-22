# glowing-spork

`glowing-spork` listens for PostgreSQL ban events and publishes localized Discord embeds to registered channels.

## What it does

- Listens to PostgreSQL notifications (`ban_added`, `ban_edited`).
- Loads full ban data from your configured bans table.
- Sends/updates Discord messages in registered channels.
- Stores channel registrations and sent message IDs in SQLite.
- Supports localization and custom embed templates.

## Function usage examples (Discord slash commands)

After starting the bot and inviting it with command permissions:

- Register current channel:
  - `/register`
- Unregister current channel:
  - `/unregister`
- Set per-channel locale override:
  - `/locale locale:en`
  - `/locale locale:ru`

## `EVENT_NAMES` usage

`EVENT_NAMES` now accepts **either** style:

- Channel names: `ban_added,ban_edited`
- Event values: `added,edited`
- Mixed values also work: `ban_added,edited`

## Detailed ban table integration example

Assume you already have a table:

```sql
CREATE TABLE public.ban_records (
  ban_id         INTEGER PRIMARY KEY,
  target_ckey    TEXT NOT NULL,
  banned_by      TEXT NOT NULL,
  ban_type       TEXT NOT NULL,
  round_ref      INTEGER,
  server_name    TEXT,
  expires_at     TIMESTAMPTZ,
  ban_reason     TEXT
);
```

Then map environment variables:

```env
BANS_TABLE=public.ban_records
BANS_COL_ID=ban_id
BANS_COL_INTRUDER=target_ckey
BANS_COL_ADMIN=banned_by
BANS_COL_TYPE=ban_type
BANS_COL_ROUND_ID=round_ref
BANS_COL_SERVER=server_name
BANS_COL_DURATION_END=expires_at
BANS_COL_REASON=ban_reason
```

At startup the app ensures and wires:

- `ban_event_type` enum (`added`, `edited`)
- `ban_events` queue table
- `notify_ban_events()` trigger function
- Trigger `<table>_notify_events_trigger` on your ban table

If any object is missing, app prompts:

```text
Create missing database object `postgres function notify_ban_events`? [y/n]:
```

You can auto-approve schema creation in non-interactive deployments:

```env
AUTO_CONFIRM_SCHEMA_CHANGES=true
```

## Environment setup

See `.env.example` for a fully documented configuration template.

## Run

```bash
just run-release
```

## Lint / checks

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```
