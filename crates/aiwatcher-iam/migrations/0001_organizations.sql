-- IAM metadata is one consistency boundary per organization. Project data and
-- artifacts do not belong in this document. Each writer locks only this row.
CREATE TABLE iam_organizations (
    id uuid PRIMARY KEY,
    document jsonb NOT NULL CHECK (jsonb_typeof(document) = 'object')
);

-- List only organizations in which a verified provider/subject is a member.
-- This is an index of the authoritative document, not a second membership store.
CREATE INDEX iam_organization_members ON iam_organizations
    USING gin ((document -> 'members') jsonb_path_ops);
