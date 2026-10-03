use super::*;

#[test]
fn previous_response_affinity_owner_repository_is_hash_only_and_route_scoped() {
    let temp_dir = TestTempDir::new("affinity_owner_hash_only");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let hash = affinity_hash('a');
    let responses_owner = PreviousResponseAffinityOwnerRecord::new(
        hash.clone(),
        account_id("acct_owner"),
        7,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_000,
    );
    let models_owner = PreviousResponseAffinityOwnerRecord::new(
        hash.clone(),
        account_id("acct_models"),
        9,
        RouteBand::Models,
        AffinitySourceTransport::WebSocket,
        1_100,
    );

    if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &responses_owner)
    {
        panic!("responses affinity owner should persist: {error}");
    }
    if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &models_owner) {
        panic!("models affinity owner should persist: {error}");
    }

    assert_eq!(
        AffinityRepository::load_previous_response_owner(
            &store,
            &hash,
            RouteBand::Responses.as_str()
        ),
        Ok(PreviousResponseAffinityOwnerLookup::Found(responses_owner))
    );
    assert_eq!(
        AffinityRepository::load_previous_response_owner(&store, &hash, RouteBand::Models.as_str()),
        Ok(PreviousResponseAffinityOwnerLookup::Found(models_owner))
    );
    assert_eq!(
        AffinityRepository::load_previous_response_owner(
            &store,
            &affinity_hash('b'),
            RouteBand::Responses.as_str()
        ),
        Ok(PreviousResponseAffinityOwnerLookup::Missing)
    );
    assert_no_previous_response_id_in_affinity_owner_rows(&database_path, "resp_raw_canary");
}

#[tokio::test]
async fn async_previous_response_affinity_owner_matches_sync_repository_projection() {
    let temp_dir = TestTempDir::new("async_affinity_owner_hash_only");
    let database_path = temp_dir.path().join("state.sqlite");
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open and migrate: {error}"),
    };
    let hash = affinity_hash('c');
    let responses_owner = PreviousResponseAffinityOwnerRecord::new(
        hash.clone(),
        account_id("acct_async_owner"),
        11,
        RouteBand::Responses,
        AffinitySourceTransport::WebSocket,
        1_200,
    );
    if let Err(error) =
        AffinityRepository::write_previous_response_owner(&sync_store, &responses_owner)
    {
        panic!("responses affinity owner should persist: {error}");
    }
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };

    let sync_lookup = match AffinityRepository::load_previous_response_owner(
        &sync_store,
        &hash,
        RouteBand::Responses.as_str(),
    ) {
        Ok(lookup) => lookup,
        Err(error) => panic!("sync affinity owner should load: {error}"),
    };
    let async_lookup = match AsyncAffinityRepository::load_previous_response_owner(
        &async_store,
        &hash,
        RouteBand::Responses.as_str(),
    )
    .await
    {
        Ok(lookup) => lookup,
        Err(error) => panic!("async affinity owner should load: {error}"),
    };
    let async_missing = match AsyncAffinityRepository::load_previous_response_owner(
        &async_store,
        &affinity_hash('d'),
        RouteBand::Responses.as_str(),
    )
    .await
    {
        Ok(lookup) => lookup,
        Err(error) => panic!("async missing affinity owner should load: {error}"),
    };

    assert_eq!(async_lookup, sync_lookup);
    assert_eq!(async_missing, PreviousResponseAffinityOwnerLookup::Missing);
}

#[tokio::test]
async fn async_previous_response_affinity_owner_write_matches_sync_projection() {
    let temp_dir = TestTempDir::new("async_affinity_owner_write");
    let database_path = temp_dir.path().join("state.sqlite");
    let async_store = match AsyncSqliteStateStore::open(&database_path).await {
        Ok(store) => store,
        Err(error) => panic!("async state store should open and migrate: {error}"),
    };
    let hash = affinity_hash('e');
    let responses_owner = PreviousResponseAffinityOwnerRecord::new(
        hash.clone(),
        account_id("acct_async_owner_write"),
        12,
        RouteBand::Responses,
        AffinitySourceTransport::HttpSse,
        1_300,
    );
    if let Err(error) =
        AsyncAffinityRepository::write_previous_response_owner(&async_store, &responses_owner).await
    {
        panic!("async affinity owner should persist: {error}");
    }
    let sync_store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("sync state store should open after async write: {error}"),
    };
    let sync_lookup = match AffinityRepository::load_previous_response_owner(
        &sync_store,
        &hash,
        RouteBand::Responses.as_str(),
    ) {
        Ok(lookup) => lookup,
        Err(error) => panic!("sync affinity owner should load after async write: {error}"),
    };

    assert_eq!(
        sync_lookup,
        PreviousResponseAffinityOwnerLookup::Found(responses_owner)
    );
}

#[test]
fn previous_response_affinity_owner_detects_ambiguous_rows_and_can_purge() {
    let temp_dir = TestTempDir::new("affinity_owner_ambiguous_purge");
    let database_path = temp_dir.path().join("state.sqlite");
    let store = match SqliteStateStore::open(&database_path) {
        Ok(store) => store,
        Err(error) => panic!("state store should open and migrate: {error}"),
    };
    let hash = affinity_hash('c');
    for account_id_value in ["acct_first", "acct_second"] {
        let owner = PreviousResponseAffinityOwnerRecord::new(
            hash.clone(),
            account_id(account_id_value),
            1,
            RouteBand::Responses,
            AffinitySourceTransport::HttpSse,
            2_000,
        );
        if let Err(error) = AffinityRepository::write_previous_response_owner(&store, &owner) {
            panic!("affinity owner should persist: {error}");
        }
    }

    assert_eq!(
        AffinityRepository::load_previous_response_owner(
            &store,
            &hash,
            RouteBand::Responses.as_str()
        ),
        Ok(PreviousResponseAffinityOwnerLookup::Ambiguous)
    );

    if let Err(error) = AffinityRepository::purge_previous_response_owners(&store) {
        panic!("affinity owners should purge: {error}");
    }
    assert_eq!(
        AffinityRepository::load_previous_response_owner(
            &store,
            &hash,
            RouteBand::Responses.as_str()
        ),
        Ok(PreviousResponseAffinityOwnerLookup::Missing)
    );
}
