use super::*;

#[tokio::test]
async fn only_reader_posts_renew_the_subscription() {
    let mut fixture = ThreadSubscriptionFixture::create("renewal-source").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let initial = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();

    let other_post_at = fixture.now + chrono::Duration::minutes(5);
    fixture
        .post_reply_as(human("other-reader"), "other post", other_post_at)
        .await;
    let after_other_post = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();
    assert_eq!(after_other_post.renewed_at, initial.renewed_at);

    let reader_post_at = fixture.now + chrono::Duration::minutes(10);
    fixture
        .post_reply_as(fixture.reader.clone(), "reader post", reader_post_at)
        .await;
    let after_reader_post = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();
    assert_eq!(after_reader_post.renewed_at, persisted_time(reader_post_at));
    fixture.finish().await;
}

#[tokio::test]
async fn explicit_renewal_extends_policy_lifetime_and_off_rows_still_expire() {
    let mut fixture = ThreadSubscriptionFixture::create("explicit-renewal").await;
    let scope = SubscriptionScope::thread(fixture.root_message_id.clone());
    let renewed_at = fixture.now + chrono::Duration::hours(1);
    let renewed = fixture
        .store
        .renew_thread_subscription(&fixture.reader, &scope, renewed_at)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed.renewed_at, persisted_time(renewed_at));
    assert_eq!(
        renewed.expires_at.signed_duration_since(renewed.renewed_at),
        chrono::Duration::hours(24)
    );
    assert_eq!(
        fixture
            .store
            .list_reader_subscriptions(&fixture.reader, renewed_at)
            .await
            .unwrap()
            .len(),
        1
    );

    fixture
        .store
        .subscribe_thread_subscription(
            ThreadSubscriptionSubscribeRequest {
                reader: fixture.reader.clone(),
                scope: scope.clone(),
                policy: policy_patch(Some(SubscriptionMode::Off), None, None, None, None),
            },
            renewed_at,
        )
        .await
        .unwrap();
    let expired_at = renewed_at + chrono::Duration::hours(25);
    assert!(
        fixture
            .store
            .list_reader_subscriptions(&fixture.reader, expired_at)
            .await
            .unwrap()
            .is_empty()
    );
    let expired = get_record(&mut fixture.store, &fixture.reader, &scope)
        .await
        .unwrap();
    assert_eq!(
        expired.state,
        SubscriptionState::Ended {
            reason: EndReason::Expired
        }
    );
    fixture.finish().await;
}
