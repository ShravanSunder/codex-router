#![allow(clippy::unwrap_used)]
mod board_behavior_support;

use board_behavior_support::*;
use project_board::*;
use project_board_storage::BoardStore;
use std::{sync::Arc, time::Instant};
use tokio::sync::{Barrier, Mutex};

#[tokio::test]
async fn concurrent_inbox_reads_preserve_summaries_and_report_latency() {
    let path = database_path("inbox-latency");
    let mut store = BoardStore::open(&path).await.unwrap();
    let fixture = create_fixture(&mut store).await;
    let root = post(
        &mut store,
        Placement::Topic {
            topic_id: fixture.topic_id,
        },
        actor("writer"),
        "Concurrent coordination",
        vec![],
    )
    .await
    .message
    .message_id;
    let readers = (0..8)
        .map(|index| actor(&format!("reader-{index}")))
        .collect::<Vec<_>>();
    for reader in &readers {
        store
            .watch_thread(ThreadWatchRequest {
                root_message_id: root.clone(),
                actor: reader.clone(),
                acting_for: None,
            })
            .await
            .unwrap();
    }
    let store = Arc::new(Mutex::new(store));
    let barrier = Arc::new(Barrier::new(readers.len() + 1));
    let mut tasks = Vec::new();
    for reader in readers.clone() {
        let store = store.clone();
        let barrier = barrier.clone();
        let project_id = fixture.project_id.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let mut observations = Vec::new();
            for _ in 0..8 {
                let waiting = Instant::now();
                let mut store = store.lock().await;
                let wait = waiting.elapsed().as_micros();
                let reading = Instant::now();
                let inbox = store
                    .fetch_inbox(InboxFetchRequest {
                        project_id: project_id.clone(),
                        reader: reader.clone(),
                        page: page(100),
                    })
                    .await
                    .unwrap();
                let summary = store
                    .list_inbox_projects(InboxProjectsRequest {
                        reader: reader.clone(),
                        unread_only: false,
                        page: page(100),
                    })
                    .await
                    .unwrap();
                assert_eq!(summary.page.records.len(), 1);
                assert_eq!(
                    summary.page.records[0].has_unread,
                    !inbox.page.records.is_empty()
                );
                observations.push((wait, reading.elapsed().as_micros()));
            }
            observations
        }));
    }
    barrier.wait().await;
    let mut write_times = Vec::new();
    let mut lock_waits = Vec::new();
    for _ in 0..16 {
        let waiting = Instant::now();
        let mut store = store.lock().await;
        lock_waits.push(waiting.elapsed().as_micros());
        let writing = Instant::now();
        post(
            &mut store,
            Placement::Thread {
                root_message_id: root.clone(),
            },
            actor("writer"),
            "A new finding",
            vec![],
        )
        .await;
        write_times.push(writing.elapsed().as_micros());
    }
    let mut read_times = Vec::new();
    for task in tasks {
        for (wait, read) in task.await.unwrap() {
            lock_waits.push(wait);
            read_times.push(read);
        }
    }
    let mut store = Arc::try_unwrap(store).ok().unwrap().into_inner();
    for reader in readers {
        let inbox = store
            .fetch_inbox(InboxFetchRequest {
                project_id: fixture.project_id.clone(),
                reader,
                page: page(100),
            })
            .await
            .unwrap();
        assert_eq!(inbox.page.records.len(), 16);
    }
    eprintln!(
        "8 readers, 16 writes: read+summary p95={}us; write p95={}us; mutex wait p95={}us",
        percentile_95(&mut read_times),
        percentile_95(&mut write_times),
        percentile_95(&mut lock_waits)
    );
    store.close().await.unwrap();
    std::fs::remove_file(path).unwrap();
}

fn percentile_95(observations: &mut [u128]) -> u128 {
    observations.sort_unstable();
    *observations
        .get((observations.len() * 95).div_ceil(100) - 1)
        .unwrap()
}
