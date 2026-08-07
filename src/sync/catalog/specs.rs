//! Category/unit inference for discovered cloud services (not a service allowlist).

pub fn infer_category(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("storage")
        || n.contains("s3")
        || n.contains("blob")
        || n.contains("disk")
        || n.contains("volume")
        || n.contains("backup")
        || n.contains("archive")
    {
        "storage"
    } else if n.contains("sql")
        || n.contains("database")
        || n.contains(" db")
        || n.contains("cache")
        || n.contains("redis")
        || n.contains("dynamo")
        || n.contains("cosmos")
        || n.contains("firestore")
        || n.contains("nosql")
    {
        "database"
    } else if n.contains("kafka")
        || n.contains("queue")
        || n.contains("event hub")
        || n.contains("pub/sub")
        || n.contains("pubsub")
        || n.contains("sns")
        || n.contains("sqs")
        || n.contains("stream")
        || n.contains("messaging")
    {
        "messaging"
    } else if n.contains("network")
        || n.contains("load balanc")
        || n.contains("cdn")
        || n.contains("dns")
        || n.contains("vpc")
        || n.contains("gateway")
        || n.contains("firewall")
        || n.contains("virtual network")
    {
        "networking"
    } else if n.contains("security")
        || n.contains("waf")
        || n.contains("key vault")
        || n.contains("key management")
        || n.contains("identity")
        || n.contains("iam")
        || n.contains("armor")
        || n.contains("defender")
        || n.contains("certificate")
    {
        "security"
    } else if n.contains("machine learning")
        || n.contains("sagemaker")
        || n.contains("vertex")
        || n.contains("openai")
        || n.contains("bedrock")
        || n.contains(" ai ")
        || n.contains("ml ")
    {
        "ai_ml"
    } else {
        "compute"
    }
}

/// Token-priced LLM APIs — use LLM / Token Usage, not infrastructure line items.
pub fn is_llm_token_service(service_key: &str) -> bool {
    matches!(
        service_key,
        "AmazonBedrockFoundationModels"
    )
}

pub fn infer_unit(name: &str) -> &'static str {
    let n = name.to_lowercase();
    if n.contains("storage")
        || n.contains("gb")
        || n.contains("data transfer")
        || n.contains("backup storage")
    {
        "gb-month"
    } else if n.contains("dynamo")
        || n.contains("sns")
        || n.contains("sqs")
        || n.contains("lambda")
        || n.contains("api gateway")
        || n.contains("request")
        || n.contains("invocation")
        || n.contains("transaction")
        || n.contains("message")
    {
        "million-invocations"
    } else {
        "hours"
    }
}

pub fn humanize_service_key(key: &str) -> String {
    key.trim_start_matches("Amazon")
        .trim_start_matches("Azure")
        .trim_start_matches("Google")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// AWS Price List attribute used to pick a configurable SKU (instance type, etc.).
pub fn aws_attr_key(offer_code: &str) -> Option<&'static str> {
    match offer_code {
        "AmazonEC2" | "AmazonRDS" | "AmazonElastiCache" | "AmazonRedshift" | "AmazonES" => {
            Some("instanceType")
        }
        _ => None,
    }
}

pub fn aws_default_sku(offer_code: &str) -> String {
    match offer_code {
        "AmazonEC2" => "t3.micro".into(),
        "AmazonRDS" => "db.t3.micro".into(),
        "AmazonElastiCache" => "cache.t3.micro".into(),
        "AmazonRedshift" => "dc2.large".into(),
        "AmazonES" => "t3.small.elasticsearch".into(),
        other => other.into(),
    }
}
