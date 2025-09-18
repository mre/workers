use std::{collections::HashMap, time::Duration};

use tokio::task::AbortHandle;

use crate::runner::{CleanupPolicy, Queue};

type QueueName = String;

/// This is in charge of cleaning up archived tasks in case retention is set to `Retention::Limited`
pub(crate) struct Cleaner {
    queue_configurations: HashMap<QueueName, CleanupPolicy>,
}

impl Cleaner {
    /// It is not necessary to return a `Cleaner` if there are no queues that need cleaning
    pub(crate) fn for_queues<_QueueContext>(
        queues: HashMap<String, Queue<_QueueContext>>,
    ) -> Option<Self> {
        let queue_configurations: HashMap<String, CleanupPolicy> = queues
            .iter()
            .filter_map(|(name, queue)| Some((name.to_owned(), queue.get_cleanup_policy()?)))
            .collect();
        if queue_configurations.is_empty() {
            return None;
        }

        Some(Self {
            queue_configurations,
        })
    }

    pub(crate) fn start(&self) -> AbortHandle {
        // TODO: consider making the interval configurable
        let task = tokio::spawn(async {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                for (queue_name, policy) in self.queue_configurations {
                    sqlx::query("DELETE FROM archived_jobs WHERE ")
                }
                
                // TODO: cleanup
            }
        });
        task.abort_handle()
    }
}
