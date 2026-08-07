#[cfg(test)]
use super::{aws, azure, gcp};
use crate::db::Database;
use tempfile::NamedTempFile;

#[ignore = "live AWS Price List API"]
#[tokio::test]
async fn live_aws_catalog_sync() {
    let file = NamedTempFile::new().unwrap();
    let db = Database::open(file.path()).unwrap();
    let count = aws::sync(&db, 8).await.unwrap();
    assert!(count > 50, "expected many AWS offers, got {count}");
    assert!(db.provider_catalog_count().unwrap() > 50);
    let hits = db.search_catalog("elasticache", Some("aws"), 5).unwrap();
    assert!(!hits.is_empty());
}

#[ignore = "live Azure Retail Prices API — full scan is slow"]
#[tokio::test]
async fn live_azure_catalog_sync() {
    let file = NamedTempFile::new().unwrap();
    let db = Database::open(file.path()).unwrap();
    let count = azure::sync_pages(&db, 5).await.unwrap();
    assert!(count > 10, "expected azure services, got {count}");
}

#[ignore = "live GCP Cloud Billing Catalog API — requires GCP_PRICING_API_KEY"]
#[tokio::test]
async fn live_gcp_catalog_sync() {
    if std::env::var("GCP_PRICING_API_KEY").is_err() {
        eprintln!("skip: GCP_PRICING_API_KEY not set");
        return;
    }
    let file = NamedTempFile::new().unwrap();
    let db = Database::open(file.path()).unwrap();
    let count = gcp::sync(&db, 8).await.unwrap();
    assert!(count > 10, "expected gcp services, got {count}");
}
