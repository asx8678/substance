# 0019 Seeding, accounts and canaries

Status: accepted · Date: 2026-10-21

## Context
Six checks need two users per role and seeded rows, and none can assume an app's schema.

## Decision

- Order: provision, boot, seed, accounts, stop, `seeded` snapshot. Seeding precedes accounts because
a repository's own seed may create an `existing` account; the stop precedes the snapshot because a
copy of a database being written is not a copy of anything.
- Seed command is `[app] seed`, else `prisma db seed`, an npm `seed` script, `supabase/seed.sql` or
Django fixtures, recording which file decided it. No convention means no seed step: inventing one
would run code nobody pointed at.
- `[[users]]` names an environment variable, never a password. Only `api` and `existing` create
accounts; `sql`, `supabase` and `ui` each report what they still need rather than guessing.
- Canaries are `cnry-<16 hex>` from `sha256(master_seed, "canary", handle)`, sorted and tagged by
owner in `canaries.json`: never from a password, since they travel into rows, URLs and logs.
- One seed per generator, `sha256(master_seed, generator)`, in `manifest.seeds` before the run id is
derived: a seed is an input, and two different seeds must be two runs.

## Consequences

Without `--allow-exec` or a database a run still reports, and every runtime check says why. `sql`
accounts need a schema template in the config, which M4 can add.