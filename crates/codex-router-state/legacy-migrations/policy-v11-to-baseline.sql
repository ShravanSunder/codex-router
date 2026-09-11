CREATE TABLE account_routing_policies_v12 (
    account_id TEXT PRIMARY KEY NOT NULL,
    weekly_quota_floor_basis_points INTEGER NOT NULL
        CHECK (
            weekly_quota_floor_basis_points BETWEEN 100 AND 1500
            AND weekly_quota_floor_basis_points % 100 = 0
        )
);

INSERT INTO account_routing_policies_v12 (
    account_id,
    weekly_quota_floor_basis_points
)
SELECT
    account_id,
    weekly_quota_floor_basis_points
FROM account_routing_policies;

DROP TABLE account_routing_policies;
ALTER TABLE account_routing_policies_v12 RENAME TO account_routing_policies;
