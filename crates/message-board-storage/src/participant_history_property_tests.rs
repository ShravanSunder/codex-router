#![allow(clippy::unwrap_used)]
//! Seeded real-API histories, checked against Roles at post time, not SQL predicates.
use crate::participant_history_test_support::*;
use crate::storage_support::identity_key;
use message_board::{ParticipantRole::*, *};
use std::collections::BTreeMap;

const HISTORY_SEED: u64 = 20261002;
const TRACE_COUNT: usize = 2;
const ACTIONS_PER_TRACE: usize = 48;

struct SeededChoices(u64);
impl SeededChoices {
    fn choose(&mut self, choices: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(choices).unwrap()).unwrap()
    }
}
struct PostedRole {
    root: MessageId,
    actor: Identity,
    role: Option<ParticipantRole>,
}
struct GrantedRole {
    root: MessageId,
    actor: Identity,
    role: ParticipantRole,
}
struct HistoryOracle {
    open_roles: [[Option<ParticipantRole>; 4]; 2],
    resolved: [bool; 2],
    grants: BTreeMap<i64, GrantedRole>,
    posts: BTreeMap<String, PostedRole>,
}
impl HistoryOracle {
    fn new() -> Self {
        Self {
            open_roles: [[None; 4]; 2],
            resolved: [false; 2],
            grants: BTreeMap::new(),
            posts: BTreeMap::new(),
        }
    }
    fn grant(
        &mut self,
        sequence: i64,
        root_index: usize,
        roots: &[MessageId; 2],
        actor_index: usize,
        actors: &[Identity; 4],
        role: ParticipantRole,
    ) {
        self.open_roles[root_index][actor_index] = Some(role);
        self.grants.insert(
            sequence,
            GrantedRole {
                root: roots[root_index].clone(),
                actor: actors[actor_index].clone(),
                role,
            },
        );
    }
    fn post(
        &mut self,
        message: &Message,
        root: &MessageId,
        actor_index: usize,
        role: Option<ParticipantRole>,
    ) {
        self.posts.insert(
            message.message_id.as_str().to_owned(),
            PostedRole {
                root: root.clone(),
                actor: message.actor.clone(),
                role: if actor_index == 3 { None } else { role },
            },
        );
    }
    async fn assert_attribution(&self, fixture: &mut HistoryFixture, live: bool) -> (usize, usize) {
        let mut known = 0;
        let mut unknown = 0;
        for (message_id, expected) in &self.posts {
            let id: MessageId = message_id.clone().try_into().unwrap();
            let pointer = posted_from(&mut fixture.store, &id).await;
            match pointer {
                Some(sequence) => {
                    let grant = self.grants.get(&sequence).unwrap();
                    assert_eq!(
                        grant.root, expected.root,
                        "seed {HISTORY_SEED}, {message_id}"
                    );
                    assert_eq!(
                        grant.actor, expected.actor,
                        "seed {HISTORY_SEED}, {message_id}"
                    );
                    assert_eq!(
                        Some(grant.role),
                        expected.role,
                        "seed {HISTORY_SEED}, {message_id}"
                    );
                    let event: (String, String, String) = sqlx::query_as("SELECT root_id,participant_key,participant_role FROM board_activity WHERE activity_sequence=?").bind(sequence).fetch_one(&mut fixture.store.connection).await.unwrap();
                    assert_eq!(
                        event,
                        (
                            grant.root.as_str().to_owned(),
                            identity_key(&grant.actor),
                            crate::participant_row_decoding::role_name(grant.role).to_owned()
                        )
                    );
                    known += 1;
                }
                None => {
                    if live {
                        assert!(
                            expected.role.is_none(),
                            "missing live grant, seed {HISTORY_SEED}, {message_id}"
                        );
                    }
                    unknown += 1;
                }
            }
            let shown = fixture
                .store
                .show_message(MessageShowRequest { message_id: id })
                .await
                .unwrap();
            assert_eq!(
                shown.0.posted_as_role,
                pointer.map(|sequence| self.grants[&sequence].role)
            );
        }
        (known, unknown)
    }
}

#[tokio::test]
async fn participant_history_seeded_lifecycle_property_matches_independent_roles_live_and_backfilled()
 {
    let started = std::time::Instant::now();
    let mut choices = SeededChoices(HISTORY_SEED);
    let actors = [session("A"), session("B"), session("O"), human("H")];
    let roles = [Orchestrator, Implementer, Reviewer, Advisor, Participant];
    let mut live_counts = (0, 0);
    let mut backfill_counts = (0, 0);
    let mut actions = [0_usize; 7];
    for _ in 0..TRACE_COUNT {
        let mut fixture = HistoryFixture::open().await;
        let roots = [
            fixture.create(human("root-0"), None).await.message_id,
            fixture.create(human("root-1"), None).await.message_id,
        ];
        let mut oracle = HistoryOracle::new();
        // P1 guarantees sensitivity to a stale grant when a later join was overwritten.
        for (actor_index, role, holder) in [
            (1, Implementer, None),
            (0, Implementer, Some(1)),
            (0, Reviewer, None),
        ] {
            let sequence = fixture
                .join(
                    &roots[0],
                    actors[actor_index].clone(),
                    role,
                    holder.map(|index| actors[index].clone()),
                )
                .await;
            if let Some(index) = holder {
                oracle.open_roles[0][index] = None;
            }
            oracle.grant(sequence, 0, &roots, actor_index, &actors, role);
        }
        let message = fixture.post(&roots[0], actors[0].clone()).await;
        oracle.post(&message, &roots[0], 0, Some(Reviewer));
        let sequence = fixture
            .join(&roots[0], actors[0].clone(), Advisor, None)
            .await;
        oracle.grant(sequence, 0, &roots, 0, &actors, Advisor);
        // P5 spans handover, resolution, rejoin, post and an overwritten join.
        for (actor_index, role) in [(2, Orchestrator), (0, Reviewer)] {
            let sequence = fixture
                .join(&roots[1], actors[actor_index].clone(), role, None)
                .await;
            oracle.grant(sequence, 1, &roots, actor_index, &actors, role);
        }
        let sequence = fixture
            .leave(&roots[1], actors[2].clone(), Some(actors[0].clone()), false)
            .await;
        oracle.open_roles[1][2] = None;
        oracle.grant(sequence, 1, &roots, 0, &actors, Orchestrator);
        let message = fixture.post(&roots[1], actors[0].clone()).await;
        oracle.post(&message, &roots[1], 0, Some(Orchestrator));
        fixture
            .leave(&roots[1], actors[0].clone(), None, true)
            .await;
        oracle.open_roles[1] = [None; 4];
        oracle.resolved[1] = true;
        fixture.unresolve(&roots[1]).await;
        oracle.resolved[1] = false;
        let sequence = fixture
            .join(&roots[1], actors[0].clone(), Reviewer, None)
            .await;
        oracle.grant(sequence, 1, &roots, 0, &actors, Reviewer);
        let message = fixture.post(&roots[1], actors[0].clone()).await;
        oracle.post(&message, &roots[1], 0, Some(Reviewer));
        let sequence = fixture
            .join(&roots[1], actors[0].clone(), Advisor, None)
            .await;
        oracle.grant(sequence, 1, &roots, 0, &actors, Advisor);
        actions[0] += 8;
        actions[2] += 3;
        actions[4] += 1;
        actions[5] += 1;
        actions[6] += 1;
        for _ in 0..ACTIONS_PER_TRACE {
            let root_index = choices.choose(2);
            let actor_index = choices.choose(4);
            let action = choices.choose(7);
            let root = &roots[root_index];
            if action == 5 {
                if !oracle.resolved[root_index] {
                    fixture.resolve(root, human("owner")).await;
                    oracle.open_roles[root_index] = [None; 4];
                    oracle.resolved[root_index] = true;
                    actions[action] += 1;
                }
                continue;
            }
            if action == 6 {
                if oracle.resolved[root_index] {
                    fixture.unresolve(root).await;
                    oracle.resolved[root_index] = false;
                    actions[action] += 1;
                }
                continue;
            }
            if oracle.resolved[root_index] {
                continue;
            }
            match action {
                0 | 1 => {
                    let role = roles[choices.choose(5)];
                    if oracle.open_roles[root_index][actor_index] == Some(Orchestrator)
                        && role != Orchestrator
                    {
                        continue;
                    }
                    let holder = if matches!(role, Orchestrator | Implementer) {
                        oracle.open_roles[root_index]
                            .iter()
                            .position(|open| *open == Some(role))
                            .filter(|index| *index != actor_index)
                    } else {
                        None
                    };
                    let sequence = fixture
                        .join(
                            root,
                            actors[actor_index].clone(),
                            role,
                            holder.map(|index| actors[index].clone()),
                        )
                        .await;
                    if let Some(index) = holder {
                        oracle.open_roles[root_index][index] = None;
                    }
                    oracle.grant(sequence, root_index, &roots, actor_index, &actors, role);
                    actions[action] += 1;
                    let message = fixture.post(root, actors[actor_index].clone()).await;
                    oracle.post(&message, root, actor_index, Some(role));
                    actions[2] += 1;
                }
                2 => {
                    let role = oracle.open_roles[root_index][actor_index];
                    if actor_index != 3 && role.is_none() {
                        continue;
                    }
                    let message = fixture.post(root, actors[actor_index].clone()).await;
                    oracle.post(&message, root, actor_index, role);
                    actions[action] += 1;
                }
                3 => {
                    if oracle.open_roles[root_index][actor_index].is_none()
                        || oracle.open_roles[root_index][actor_index] == Some(Orchestrator)
                    {
                        continue;
                    }
                    fixture
                        .leave(root, actors[actor_index].clone(), None, false)
                        .await;
                    oracle.open_roles[root_index][actor_index] = None;
                    actions[action] += 1;
                }
                4 => {
                    let Some(holder) = oracle.open_roles[root_index]
                        .iter()
                        .position(|role| *role == Some(Orchestrator))
                    else {
                        continue;
                    };
                    if holder == actor_index || oracle.open_roles[root_index][actor_index].is_none()
                    {
                        continue;
                    }
                    let sequence = fixture
                        .leave(
                            root,
                            actors[holder].clone(),
                            Some(actors[actor_index].clone()),
                            false,
                        )
                        .await;
                    oracle.open_roles[root_index][holder] = None;
                    oracle.grant(
                        sequence,
                        root_index,
                        &roots,
                        actor_index,
                        &actors,
                        Orchestrator,
                    );
                    actions[action] += 1;
                }
                _ => assert!(action < 7, "invalid generated action"),
            }
        }
        // Preserve a recoverable final grant and a joined human post in every trace.
        if oracle.resolved[0] {
            fixture.unresolve(&roots[0]).await;
            oracle.resolved[0] = false;
            actions[6] += 1;
        }
        for actor_index in [0, 3] {
            let role = if oracle.open_roles[0][actor_index] == Some(Orchestrator) {
                Orchestrator
            } else {
                Reviewer
            };
            let sequence = fixture
                .join(&roots[0], actors[actor_index].clone(), role, None)
                .await;
            oracle.grant(sequence, 0, &roots, actor_index, &actors, role);
            actions[0] += 1;
            let message = fixture.post(&roots[0], actors[actor_index].clone()).await;
            oracle.post(&message, &roots[0], actor_index, Some(role));
            actions[2] += 1;
        }
        let counts = oracle.assert_attribution(&mut fixture, true).await;
        live_counts.0 += counts.0;
        live_counts.1 += counts.1;
        sqlx::raw_sql("UPDATE board_messages SET posted_from_activity=NULL; UPDATE board_activity SET participant_key=NULL,participant_role=NULL,replaced_participant_key=NULL;").execute(&mut fixture.store.connection).await.unwrap();
        let migration = include_str!("../migrations/202610020001_participant_history.sql");
        let backfill = migration.split_once("-- Step 1:").unwrap().1;
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("-- Step 1:{backfill}")))
            .execute(&mut fixture.store.connection)
            .await
            .unwrap();
        let counts = oracle.assert_attribution(&mut fixture, false).await;
        backfill_counts.0 += counts.0;
        backfill_counts.1 += counts.1;
        assert!(
            sqlx::query("PRAGMA foreign_key_check")
                .fetch_all(&mut fixture.store.connection)
                .await
                .unwrap()
                .is_empty()
        );
    }
    assert!(
        live_counts.0 > 0 && live_counts.1 > 0 && backfill_counts.0 > 0 && backfill_counts.1 > 0
    );
    eprintln!(
        "seed={HISTORY_SEED}; traces={TRACE_COUNT}; actions={actions:?}; live={live_counts:?}; backfilled={backfill_counts:?}; elapsed={:?}",
        started.elapsed()
    );
}
