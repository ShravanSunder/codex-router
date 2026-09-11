DROP INDEX IF EXISTS active_client_leases_account_lookup;

CREATE TABLE active_client_leases_v8 (
    route_band TEXT NOT NULL,
    process_run_id TEXT NOT NULL,
    reservation_id TEXT NOT NULL,
    account_id TEXT NOT NULL,
    acquired_unix_seconds INTEGER NOT NULL,
    active_pressure INTEGER NOT NULL,
    PRIMARY KEY (route_band, process_run_id, reservation_id)
);

INSERT INTO active_client_leases_v8 (
    route_band,
    process_run_id,
    reservation_id,
    account_id,
    acquired_unix_seconds,
    active_pressure
)
SELECT
    route_band,
    'legacy',
    reservation_id,
    account_id,
    acquired_unix_seconds,
    8
FROM active_client_leases;

DROP TABLE active_client_leases;
ALTER TABLE active_client_leases_v8 RENAME TO active_client_leases;
