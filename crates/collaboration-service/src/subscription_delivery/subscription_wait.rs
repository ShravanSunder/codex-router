use message_board::{
    BoardError, MessageId, SubscriptionBatch, SubscriptionMode, SubscriptionScope,
    ThreadSubscriptionRecord, TopicId,
};
use message_board_storage::BoardStore;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{Mutex, oneshot};
use tokio::time::Instant;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubscriptionWaitFilter {
    All,
    Roots(Vec<MessageId>),
    Topic(TopicId),
}

#[derive(Clone, Debug)]
pub enum SubscriptionWaitResult {
    Notice {
        push_id: collaboration_protocol::PushId,
        line: collaboration_protocol::MessageText,
        batch: SubscriptionBatch,
    },
    Ranges {
        batch: SubscriptionBatch,
    },
}

impl SubscriptionWaitFilter {
    pub(super) fn validate(&self) -> Result<(), BoardError> {
        if let Self::Roots(roots) = self
            && (roots.is_empty() || roots.iter().collect::<HashSet<_>>().len() != roots.len())
        {
            return Err(BoardError::invalid_field(
                "rootMessageIds",
                "must contain at least one distinct root",
            ));
        }
        Ok(())
    }

    pub(super) async fn matches(
        &self,
        root: &MessageId,
        store: &Arc<Mutex<BoardStore>>,
    ) -> Result<bool, BoardError> {
        match self {
            Self::All => Ok(true),
            Self::Roots(roots) => Ok(roots.contains(root)),
            Self::Topic(topic) => Ok(store
                .lock()
                .await
                .show_message(message_board::MessageShowRequest {
                    message_id: root.clone(),
                })
                .await?
                .0
                .topic_id
                == *topic),
        }
    }

    pub(super) async fn covered_poll_scopes(
        &self,
        records: &[ThreadSubscriptionRecord],
        store: &Arc<Mutex<BoardStore>>,
    ) -> Result<Vec<SubscriptionScope>, BoardError> {
        let mut scopes = Vec::new();
        for record in records
            .iter()
            .filter(|record| record.policy().mode() == SubscriptionMode::Poll)
        {
            let covered = match (self, record.scope()) {
                (Self::All, _) => true,
                (Self::Topic(topic), SubscriptionScope::Topic { topic_id }) => topic == topic_id,
                (Self::Topic(_), SubscriptionScope::Thread { root_message_id }) => {
                    self.matches(root_message_id, store).await?
                }
                (Self::Roots(roots), SubscriptionScope::Thread { root_message_id }) => {
                    roots.contains(root_message_id)
                }
                (Self::Roots(roots), SubscriptionScope::Topic { topic_id }) => {
                    let mut found = false;
                    for root in roots {
                        let mut store = store.lock().await;
                        if store
                            .get_thread_subscription_record(
                                record.reader(),
                                &SubscriptionScope::thread(root.clone()),
                            )
                            .await?
                            .is_some()
                        {
                            continue;
                        }
                        found |= store
                            .show_message(message_board::MessageShowRequest {
                                message_id: root.clone(),
                            })
                            .await?
                            .0
                            .topic_id
                            == *topic_id;
                    }
                    found
                }
            };
            if covered {
                scopes.push(record.scope().clone());
            }
        }
        if scopes.is_empty() {
            return Err(BoardError::invalid_field(
                "filter",
                "wait requires a poll subscription; use board thread subscribe --mode poll",
            ));
        }
        Ok(scopes)
    }
}

pub(super) struct PollWaiter {
    pub filter: SubscriptionWaitFilter,
    pub deadline: Instant,
    pub maximum_root_notice_bytes: usize,
    pub reply: oneshot::Sender<Result<Option<SubscriptionWaitResult>, BoardError>>,
}
