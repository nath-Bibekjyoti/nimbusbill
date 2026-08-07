use crate::models::{EstimateRequest, RequirementSet, ResourceSpec};
use anyhow::{Context, Result, bail};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::path::Path;

pub enum ParsedUpload {
    Requirement(RequirementSet),
    Estimate(EstimateRequest),
}

pub fn parse_file(path: &Path) -> Result<RequirementSet> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    match parse_upload_bytes(
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload"),
        &bytes,
    )? {
        ParsedUpload::Requirement(set) => Ok(set),
        ParsedUpload::Estimate(req) => estimate_to_requirement(req),
    }
}

pub fn parse_upload_bytes(filename: &str, bytes: &[u8]) -> Result<ParsedUpload> {
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "json" => parse_json_upload(bytes),
        "yaml" | "yml" => Ok(ParsedUpload::Requirement(parse_yaml_str(
            std::str::from_utf8(bytes).context("yaml must be utf-8")?,
        )?)),
        "csv" => Ok(ParsedUpload::Requirement(parse_csv_bytes(bytes)?)),
        "xlsx" => Ok(ParsedUpload::Requirement(parse_xlsx_bytes(bytes)?)),
        "txt" | "text" => Ok(ParsedUpload::Requirement(parse_text(
            std::str::from_utf8(bytes).context("text must be utf-8")?,
        )?)),
        _ => bail!("unsupported format: {ext} (use json, yaml, csv, or xlsx)"),
    }
}

fn parse_json_upload(bytes: &[u8]) -> Result<ParsedUpload> {
    let content = std::str::from_utf8(bytes).context("json must be utf-8")?;
    if let Ok(req) = serde_json::from_str::<EstimateRequest>(content) {
        if !req.providers.is_empty() && !req.resources.is_empty() {
            return Ok(ParsedUpload::Estimate(req));
        }
    }
    Ok(ParsedUpload::Requirement(parse_json_str(content)?))
}

pub fn parse_json_str(content: &str) -> Result<RequirementSet> {
    serde_json::from_str(content).context("parse json requirement set")
}

pub fn parse_yaml_str(content: &str) -> Result<RequirementSet> {
    serde_yaml::from_str(content).context("parse yaml requirement set")
}

fn estimate_to_requirement(req: EstimateRequest) -> Result<RequirementSet> {
    let resources = req
        .resources
        .into_iter()
        .map(|r| ResourceSpec {
            service: r.catalog_id.clone(),
            sku: r.sku,
            region: r.region,
            quantity: r.quantity,
            unit: "units".into(),
            provider: Some(r.provider.as_str().to_string()),
            tags: vec![],
            catalog_id: Some(r.catalog_id),
            display_name: None,
            category: None,
            sub_region: r.sub_region,
            region_label: None,
            instance_count: r.instance_count,
            hours: r.hours,
        })
        .collect();
    Ok(RequirementSet {
        name: req.name,
        description: None,
        resources,
        token_usage: req.token_usage,
    })
}

/// CSV workload: header row with columns such as
/// provider,service,catalog_id,region,sub_region,sku,quantity,instance_count,hours,unit
pub fn parse_csv_bytes(bytes: &[u8]) -> Result<RequirementSet> {
    let mut rdr = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(bytes);
    let headers = rdr
        .headers()
        .context("csv header row")?
        .iter()
        .map(|h| h.trim().to_lowercase())
        .collect::<Vec<_>>();

    let mut resources = Vec::new();
    for (i, row) in rdr.records().enumerate() {
        let row = row.with_context(|| format!("csv row {}", i + 2))?;
        if row.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let values: Vec<String> = row.iter().map(|s| s.to_string()).collect();
        let map = row_to_map(&headers, &values);
        resources.push(parse_row_resource(&map).with_context(|| format!("csv row {}", i + 2))?);
    }

    if resources.is_empty() {
        bail!("csv has no resource rows");
    }

    Ok(RequirementSet {
        name: "csv-import".into(),
        description: None,
        resources,
        token_usage: vec![],
    })
}

pub fn parse_xlsx_bytes(bytes: &[u8]) -> Result<RequirementSet> {
    use calamine::{Reader, Xlsx, open_workbook_from_rs};
    use std::io::Cursor;

    let mut workbook: Xlsx<_> =
        open_workbook_from_rs(Cursor::new(bytes)).context("open xlsx workbook")?;
    let sheet = workbook
        .sheet_names()
        .first()
        .cloned()
        .context("xlsx has no sheets")?;
    let range = workbook
        .worksheet_range(&sheet)
        .context("read xlsx sheet")?;
    let mut rows = range.rows();
    let header_row = rows
        .next()
        .context("xlsx header row")?
        .iter()
        .map(cell_to_string)
        .map(|s| s.trim().to_lowercase())
        .collect::<Vec<_>>();

    let mut resources = Vec::new();
    for (i, row) in rows.enumerate() {
        let values = row.iter().map(cell_to_string).collect::<Vec<_>>();
        if values.iter().all(|c| c.trim().is_empty()) {
            continue;
        }
        let map = row_to_map(&header_row, &values);
        resources.push(parse_row_resource(&map).with_context(|| format!("xlsx row {}", i + 2))?);
    }

    if resources.is_empty() {
        bail!("xlsx has no resource rows");
    }

    Ok(RequirementSet {
        name: "xlsx-import".into(),
        description: None,
        resources,
        token_usage: vec![],
    })
}

fn cell_to_string(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(s) => s.clone(),
        calamine::Data::Float(f) => f.to_string(),
        calamine::Data::Int(n) => n.to_string(),
        calamine::Data::Bool(b) => b.to_string(),
        calamine::Data::DateTime(d) => d.to_string(),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => s.clone(),
        calamine::Data::Error(e) => format!("{e:?}"),
    }
}

fn row_to_map(headers: &[String], row: &[String]) -> HashMap<String, String> {
    headers
        .iter()
        .zip(row.iter())
        .map(|(h, v)| (h.clone(), v.trim().to_string()))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn parse_row_resource(map: &HashMap<String, String>) -> Result<ResourceSpec> {
    let get = |key: &str| map.get(key).map(String::as_str);
    let provider = get("provider").map(str::to_string);
    let catalog_id = get("catalog_id").map(str::to_string);
    let service = get("service")
        .map(str::to_string)
        .or_else(|| catalog_id.clone())
        .context("csv/xlsx row needs service or catalog_id")?;
    let region = get("region").context("region column required")?;
    let quantity: Decimal = get("quantity")
        .context("quantity column required")?
        .parse()
        .context("quantity must be a number")?;
    let unit = get("unit").unwrap_or("hours").to_string();
    let instance_count = get("instance_count")
        .map(|s| s.parse().context("instance_count"))
        .transpose()?;
    let hours = get("hours")
        .map(|s| s.parse().context("hours"))
        .transpose()?;

    Ok(ResourceSpec {
        service,
        sku: get("sku").map(str::to_string),
        region: region.to_string(),
        quantity,
        unit,
        provider,
        tags: vec![],
        catalog_id,
        display_name: get("display_name").map(str::to_string),
        category: get("category").map(str::to_string),
        sub_region: get("sub_region").map(str::to_string),
        region_label: None,
        instance_count,
        hours,
    })
}

/// Minimal text format: one resource per line → service,region,quantity[,sku]
pub fn parse_text(content: &str) -> Result<RequirementSet> {
    let mut resources = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<_> = line.split(',').map(str::trim).collect();
        if parts.len() < 3 {
            bail!("line {}: expected service,region,quantity[,sku]", i + 1);
        }
        resources.push(ResourceSpec {
            service: parts[0].into(),
            region: parts[1].into(),
            quantity: parts[2].parse::<Decimal>().context("quantity")?,
            sku: parts.get(3).map(|s| (*s).into()),
            unit: "units".into(),
            provider: None,
            tags: vec![],
            catalog_id: None,
            display_name: None,
            category: None,
            sub_region: None,
            region_label: None,
            instance_count: None,
            hours: None,
        });
    }

    Ok(RequirementSet {
        name: "text-import".into(),
        description: None,
        resources,
        token_usage: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_csv_workload() {
        let csv = b"provider,service,region,sku,quantity,instance_count,hours\naws,ec2,us-east-1,t3.medium,730,1,730\n";
        let set = parse_csv_bytes(csv).unwrap();
        assert_eq!(set.resources.len(), 1);
        assert_eq!(set.resources[0].region, "us-east-1");
    }
}
