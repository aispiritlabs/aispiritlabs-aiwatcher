-- Where an organization's audit trail begins, once a sweep has moved it.
--
-- A table rather than an entry in `iam_audit`, and rather than a field in the
-- organization document, because both of those are read by binaries already
-- released: a new `AuditAction` variant fails an older reader's deserialization,
-- and the document is `deny_unknown_fields`. Nothing released reads this table,
-- so adding it changes nothing an older binary can see.
--
-- One row per organization. It is written in the same transaction as the delete
-- it describes, so there is no moment at which entries are gone and nothing
-- says so.
CREATE TABLE IF NOT EXISTS iam_audit_retention (
    organization_id uuid PRIMARY KEY REFERENCES iam_organizations(id),
    -- The highest sequence removed. Everything a reader can still page begins
    -- after it, and it never moves backwards.
    pruned_through_sequence bigint NOT NULL CHECK (pruned_through_sequence > 0),
    -- The cutoff of the sweep that last moved it.
    pruned_before bigint NOT NULL,
    policy_id text NOT NULL DEFAULT '',
    ttl_days integer NOT NULL CHECK (ttl_days > 0),
    pruned_at bigint NOT NULL,
    removed_total bigint NOT NULL CHECK (removed_total >= 0)
);

-- The sweep asks one question — "which entries are older than this moment" —
-- of a table whose only index is `(organization_id, sequence)`. Without this it
-- is a scan of every retained entry on every tick, and the ticks that find
-- nothing are the common case.
--
-- The expression is immutable (`->>` on jsonb and `text::bigint` both are), and
-- it is deliberately not keyed by organization first: the sweep is one
-- statement across every organization, because the cutoff is the deployment's
-- and not one organization's.
CREATE INDEX IF NOT EXISTS iam_audit_occurred_at
    ON iam_audit (((entry ->> 'occurred_at')::bigint));
