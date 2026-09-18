-- An invitation is redeemed by somebody who holds a token and knows nothing
-- else: no organization, no project, no id. So the only way into the right row
-- is the digest, and without this index that is a sequential scan of every
-- organization on every redemption.
--
-- Additive: it indexes a key inside the document that older documents simply do
-- not have, and adds no table and no column.
CREATE INDEX iam_organization_invitations ON iam_organizations
    USING gin ((document -> 'invitations') jsonb_path_ops);
