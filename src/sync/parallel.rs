//! Concurrent async workers for I/O-bound catalog and price sync.
//!
//! ponytail: multi-**process** is intentionally avoided — SQLite is single-writer;
//! parallel **async** HTTP + a shared connection mutex is the fast path on all OSes.

use anyhow::Result;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// Effective worker limit (`NIMUSBILL_CONCURRENCY` or config, clamped 1–64).
pub fn concurrency_limit(configured: usize) -> usize {
    if configured > 0 {
        return configured.clamp(1, 64);
    }
    std::env::var("NIMUSBILL_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(default_workers)
        .clamp(1, 64)
}

fn default_workers() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(4, 32))
        .unwrap_or(16)
}

/// Run `f` concurrently over `items` with at most `limit` in flight.
pub async fn for_each<I, T, F, Fut>(items: I, limit: usize, f: F) -> Result<()>
where
    I: IntoIterator<Item = T>,
    T: Send + 'static,
    F: Fn(T) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    let limit = limit.clamp(1, 64);
    let sem = Arc::new(Semaphore::new(limit));
    let f = Arc::new(f);
    let mut set = JoinSet::new();

    for item in items {
        let permit = sem.clone().acquire_owned().await?;
        let f = Arc::clone(&f);
        set.spawn(async move {
            let _permit = permit;
            f(item).await
        });
    }

    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(e),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
