ALTER TABLE credential_maintenance
    ADD COLUMN claim_purpose TEXT;

ALTER TABLE credential_maintenance
    ADD COLUMN claim_started_unix_seconds INTEGER;

ALTER TABLE credential_maintenance
    ADD COLUMN claim_prior_state TEXT;

-- Claims created before purpose was durable may already have consumed a refresh
-- token, so classify them conservatively as old refresh claims with no age proof.
UPDATE credential_maintenance
   SET claim_purpose = 'refresh',
       claim_started_unix_seconds = 0
 WHERE state = 'in_progress';
