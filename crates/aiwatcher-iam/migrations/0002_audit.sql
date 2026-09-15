CREATE TABLE iam_audit (
    organization_id uuid NOT NULL REFERENCES iam_organizations(id),
    sequence bigint NOT NULL CHECK (sequence > 0),
    entry jsonb NOT NULL CHECK (jsonb_typeof(entry) = 'object'),
    PRIMARY KEY (organization_id, sequence)
);
