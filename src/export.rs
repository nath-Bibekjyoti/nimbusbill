use crate::models::{CloudProvider, CostRow, CostTable, PeriodBreakdown, UiEstimateResponse};
use anyhow::{Context, Result};
use printpdf::{IndirectFontRef, Mm, PdfLayerReference};
use rust_decimal::Decimal;
use serde::Deserialize;
use std::io::Cursor;

const PERIOD_LABELS: [&str; 5] = ["Daily", "Monthly", "Quarterly", "Half-Yearly", "Yearly"];

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Xlsx,
    Pdf,
}

#[derive(Debug, Deserialize)]
pub struct ExportRequest {
    pub format: ExportFormat,
    pub estimate: UiEstimateResponse,
}

pub struct ExportFile {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
    pub filename: String,
}

pub fn export(req: &ExportRequest) -> Result<ExportFile> {
    let stem = format!(
        "nimbusbill-estimate-{}",
        req.estimate.created_at.format("%Y%m%d-%H%M%S")
    );
    match req.format {
        ExportFormat::Csv => Ok(ExportFile {
            bytes: to_csv(&req.estimate)?,
            content_type: "text/csv",
            filename: format!("{stem}.csv"),
        }),
        ExportFormat::Xlsx => Ok(ExportFile {
            bytes: to_xlsx(&req.estimate)?,
            content_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            filename: format!("{stem}.xlsx"),
        }),
        ExportFormat::Pdf => Ok(ExportFile {
            bytes: to_pdf(&req.estimate)?,
            content_type: "application/pdf",
            filename: format!("{stem}.pdf"),
        }),
    }
}

fn to_csv(est: &UiEstimateResponse) -> Result<Vec<u8>> {
    let mut w = csv::WriterBuilder::new()
        .flexible(true)
        .from_writer(Vec::new());
    w.write_record(["NimbusBill — Cost Report"])?;
    w.write_record(["Name", &est.name])?;
    w.write_record([
        "Generated",
        &est.created_at.format("%Y-%m-%d %H:%M UTC").to_string(),
    ])?;
    w.write_record([
        "Pricing Source",
        if est.live_pricing {
            "Live capture"
        } else {
            "Cached database"
        },
    ])?;
    w.write_record(&[] as &[&str])?;

    for pe in &est.providers {
        w.write_record([provider_label(pe.provider)])?;
        write_cost_table_csv(&mut w, "Infrastructure", &pe.infrastructure)?;
        if !pe.tokens.rows.is_empty() {
            write_cost_table_csv(
                &mut w,
                &format!("LLM / Tokens ({})", provider_label(pe.provider)),
                &pe.tokens,
            )?;
        }
        w.write_record(totals_record("Combined Total (Infra + LLM)", &pe.combined))?;
        w.write_record(&[] as &[&str])?;
    }
    Ok(w.into_inner()?)
}

fn write_cost_table_csv(w: &mut csv::Writer<Vec<u8>>, section: &str, table: &CostTable) -> Result<()> {
    w.write_record([section])?;
    w.write_record([
        "Category",
        "Service",
        "Unit Price",
        "Usage",
        "Unit",
        "Daily",
        "Monthly",
        "Quarterly",
        "Half-Yearly",
        "Yearly",
    ])?;
    for row in &table.rows {
        w.write_record(row_record(row))?;
    }
    w.write_record(totals_record("Subtotal", &table.totals))?;
    Ok(())
}

fn row_record(row: &CostRow) -> Vec<String> {
    vec![
        row.category.clone(),
        row.service.clone(),
        dec_str(row.unit_price),
        if row.usage_display.is_empty() {
            dec_str(row.quantity)
        } else {
            row.usage_display.clone()
        },
        row.unit.clone(),
        dec_str(row.costs.daily),
        dec_str(row.costs.monthly),
        dec_str(row.costs.quarterly),
        dec_str(row.costs.half_yearly),
        dec_str(row.costs.yearly),
    ]
}

fn totals_record(label: &str, costs: &PeriodBreakdown) -> Vec<String> {
    vec![
        label.into(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        dec_str(costs.daily),
        dec_str(costs.monthly),
        dec_str(costs.quarterly),
        dec_str(costs.half_yearly),
        dec_str(costs.yearly),
    ]
}

fn to_xlsx(est: &UiEstimateResponse) -> Result<Vec<u8>> {
    use rust_xlsxwriter::{Color, Format, Workbook, XlsxError};

    fn xlsx_err(e: XlsxError) -> anyhow::Error {
        anyhow::anyhow!("{e}")
    }

    let mut wb = Workbook::new();
    let bold = Format::new().set_bold();
    let header = Format::new()
        .set_bold()
        .set_background_color(Color::RGB(0x1A2332))
        .set_font_color(Color::White);
    let money = Format::new().set_num_format("$#,##0.00");

    for pe in &est.providers {
        let ws = wb.add_worksheet();
        ws.set_name(&provider_label(pe.provider)).map_err(xlsx_err)?;
        let mut row = 0u32;
        row = write_xlsx_meta(ws, row, est, &bold).map_err(xlsx_err)?;
        row += 1;
        row = write_xlsx_table(ws, row, "Infrastructure Costs", &pe.infrastructure, &header, &money, &bold)
            .map_err(xlsx_err)?;
        if !pe.tokens.rows.is_empty() {
            row += 1;
            row = write_xlsx_table(
                ws,
                row,
                &format!("LLM / Token Costs ({})", provider_label(pe.provider)),
                &pe.tokens,
                &header,
                &money,
                &bold,
            )
            .map_err(xlsx_err)?;
        }
        row += 1;
        write_xlsx_combined(ws, row, &pe.combined, &header, &money, &bold).map_err(xlsx_err)?;
    }

    let summary = wb.add_worksheet();
    summary.set_name("Summary").map_err(xlsx_err)?;
    summary
        .write_string_with_format(0, 0, "NimbusBill — Summary", &bold)
        .map_err(xlsx_err)?;
    summary
        .write_string(1, 0, &format!("Report: {}", est.name))
        .map_err(xlsx_err)?;
    summary
        .write_string(
            2,
            0,
            &format!("Generated: {}", est.created_at.format("%Y-%m-%d %H:%M UTC")),
        )
        .map_err(xlsx_err)?;

    let mut r = 4u32;
    summary.write_string_with_format(r, 0, "Provider", &header).map_err(xlsx_err)?;
    for (i, label) in PERIOD_LABELS.iter().enumerate() {
        summary
            .write_string_with_format(r, (i + 1) as u16, *label, &header)
            .map_err(xlsx_err)?;
    }
    r += 1;
    for pe in &est.providers {
        summary
            .write_string(r, 0, &provider_label(pe.provider))
            .map_err(xlsx_err)?;
        write_period_numbers(summary, r, 1, &pe.combined, &money).map_err(xlsx_err)?;
        r += 1;
    }

    wb.save_to_buffer().map_err(xlsx_err)
}

fn write_xlsx_meta(
    ws: &mut rust_xlsxwriter::Worksheet,
    mut row: u32,
    est: &UiEstimateResponse,
    _bold: &rust_xlsxwriter::Format,
) -> Result<u32, rust_xlsxwriter::XlsxError> {
    ws.write_string(row, 0, &format!("Report: {}", est.name))?;
    row += 1;
    ws.write_string(
        row,
        0,
        &format!("Generated: {}", est.created_at.format("%Y-%m-%d %H:%M UTC")),
    )?;
    row += 1;
    ws.write_string(
        row,
        0,
        &format!(
            "Pricing: {}",
            if est.live_pricing { "Live" } else { "Cached" }
        ),
    )?;
    Ok(row + 1)
}

fn write_xlsx_table(
    ws: &mut rust_xlsxwriter::Worksheet,
    mut row: u32,
    title: &str,
    table: &CostTable,
    header: &rust_xlsxwriter::Format,
    money: &rust_xlsxwriter::Format,
    bold: &rust_xlsxwriter::Format,
) -> Result<u32, rust_xlsxwriter::XlsxError> {
    ws.write_string_with_format(row, 0, title, bold)?;
    row += 1;
    let headers = [
        "Category",
        "Service",
        "Unit Price",
        "Usage",
        "Unit",
        "Daily",
        "Monthly",
        "Quarterly",
        "Half-Yearly",
        "Yearly",
    ];
    for (c, h) in headers.iter().enumerate() {
        ws.write_string_with_format(row, c as u16, *h, header)?;
    }
    row += 1;
    for item in &table.rows {
        ws.write_string(row, 0, &item.category)?;
        ws.write_string(row, 1, &item.service)?;
        ws.write_number_with_format(row, 2, dec_f64(item.unit_price), money)?;
        let usage = if item.usage_display.is_empty() {
            dec_str(item.quantity)
        } else {
            item.usage_display.clone()
        };
        ws.write_string(row, 3, &usage)?;
        ws.write_string(row, 4, &item.unit)?;
        write_period_numbers(ws, row, 5, &item.costs, money)?;
        row += 1;
    }
    ws.write_string_with_format(row, 0, "Subtotal", bold)?;
    write_period_numbers(ws, row, 5, &table.totals, money)?;
    Ok(row + 1)
}

fn write_xlsx_combined(
    ws: &mut rust_xlsxwriter::Worksheet,
    mut row: u32,
    combined: &PeriodBreakdown,
    header: &rust_xlsxwriter::Format,
    money: &rust_xlsxwriter::Format,
    bold: &rust_xlsxwriter::Format,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    ws.write_string_with_format(row, 0, "Total (Infrastructure + Tokens)", bold)?;
    row += 1;
    for (i, label) in PERIOD_LABELS.iter().enumerate() {
        ws.write_string_with_format(row, i as u16, *label, header)?;
    }
    row += 1;
    ws.write_string_with_format(row, 0, "Grand Total", bold)?;
    write_period_numbers(ws, row, 1, combined, money)?;
    Ok(())
}

fn write_period_numbers(
    ws: &mut rust_xlsxwriter::Worksheet,
    row: u32,
    col_start: u16,
    costs: &PeriodBreakdown,
    money: &rust_xlsxwriter::Format,
) -> Result<(), rust_xlsxwriter::XlsxError> {
    let vals = [
        costs.daily,
        costs.monthly,
        costs.quarterly,
        costs.half_yearly,
        costs.yearly,
    ];
    for (i, v) in vals.iter().enumerate() {
        ws.write_number_with_format(row, col_start + i as u16, dec_f64(*v), money)?;
    }
    Ok(())
}

fn to_pdf(est: &UiEstimateResponse) -> Result<Vec<u8>> {
    use printpdf::*;
    use std::io::BufWriter;

    let (doc, page1, layer1) = PdfDocument::new(
        "NimbusBill Report",
        Mm(297.0),
        Mm(210.0),
        "Layer 1",
    );
    let font = doc.add_builtin_font(BuiltinFont::Helvetica)?;
    let font_bold = doc.add_builtin_font(BuiltinFont::HelveticaBold)?;

    let mut page = page1;
    let mut layer = layer1;
    let mut y = Mm(190.0);
    let left = Mm(12.0);
    let line = Mm(6.0);

    {
        let l = doc.get_page(page).get_layer(layer);
        y = pdf_line(&l, &font_bold, 14.0, left, y, "NimbusBill — Cost Report");
        y = pdf_line(
            &l,
            &font,
            9.0,
            left,
            y - line,
            &format!(
                "Report: {}  |  Generated: {}",
                est.name,
                est.created_at.format("%Y-%m-%d %H:%M UTC")
            ),
        );
        y = pdf_line(
            &l,
            &font,
            9.0,
            left,
            y - line,
            &format!(
                "Pricing: {}",
                if est.live_pricing {
                    "Live capture"
                } else {
                    "Cached database"
                }
            ),
        );
        y -= line;
    }

    for pe in est.providers.iter().filter(|pe| {
        !pe.infrastructure.rows.is_empty() || !pe.tokens.rows.is_empty()
    }) {
        if y < Mm(30.0) {
            let (p, ly) = doc.add_page(Mm(297.0), Mm(210.0), "Page");
            page = p;
            layer = ly;
            y = Mm(190.0);
        }
        let l = doc.get_page(page).get_layer(layer);
        y = pdf_line(&l, &font_bold, 12.0, left, y, &provider_label(pe.provider));
        y -= line * 0.5;
        y = pdf_table(&l, &font, &font_bold, left, y, line, "Infrastructure", &pe.infrastructure);
        if !pe.tokens.rows.is_empty() {
            y -= line * 0.5;
            y = pdf_table(
                &l,
                &font,
                &font_bold,
                left,
                y,
                line,
                &format!("LLM / Tokens ({})", provider_label(pe.provider)),
                &pe.tokens,
            );
            y -= line * 0.5;
            y = pdf_combined(&l, &font_bold, left, y, line, &pe.combined);
        }
        y -= line * 1.5;
    }

    let mut buf = BufWriter::new(Cursor::new(Vec::new()));
    doc.save(&mut buf).context("save pdf")?;
    Ok(buf.into_inner()?.into_inner())
}

fn pdf_line(
    layer: &PdfLayerReference,
    font: &IndirectFontRef,
    size: f32,
    x: Mm,
    y: Mm,
    text: &str,
) -> Mm {
    layer.use_text(text, size, x, y, font);
    y
}

fn pdf_table(
    layer: &PdfLayerReference,
    font: &IndirectFontRef,
    font_bold: &IndirectFontRef,
    left: Mm,
    mut y: Mm,
    line: Mm,
    title: &str,
    table: &CostTable,
) -> Mm {
    y = pdf_line(layer, font_bold, 10.0, left, y, title);
    y -= Mm(2.0);
    pdf_row(layer, font_bold, 8.0, left, y, &[
        "Category".into(),
        "Service".into(),
        "Daily".into(),
        "Monthly".into(),
        "Quarterly".into(),
        "Half-Yr".into(),
        "Yearly".into(),
    ]);
    y -= line;

    for row in &table.rows {
        pdf_row(
            layer,
            font,
            8.0,
            left,
            y,
            &[
                truncate(&row.category, 12),
                truncate(&row.service, 16),
                dec_str(row.costs.daily),
                dec_str(row.costs.monthly),
                dec_str(row.costs.quarterly),
                dec_str(row.costs.half_yearly),
                dec_str(row.costs.yearly),
            ],
        );
        y -= line;
    }

    pdf_row(
        layer,
        font_bold,
        8.0,
        left,
        y,
        &[
            "Subtotal".into(),
            String::new(),
            dec_str(table.totals.daily),
            dec_str(table.totals.monthly),
            dec_str(table.totals.quarterly),
            dec_str(table.totals.half_yearly),
            dec_str(table.totals.yearly),
        ],
    );
    y - line * 0.3
}

fn pdf_combined(
    layer: &PdfLayerReference,
    font_bold: &IndirectFontRef,
    left: Mm,
    mut y: Mm,
    line: Mm,
    combined: &PeriodBreakdown,
) -> Mm {
    y = pdf_line(
        layer,
        font_bold,
        10.0,
        left,
        y,
        "Total (Infrastructure + Tokens)",
    );
    pdf_row(
        layer,
        font_bold,
        8.0,
        left,
        y - line * 0.5,
        &[
            "Grand Total".into(),
            String::new(),
            dec_str(combined.daily),
            dec_str(combined.monthly),
            dec_str(combined.quarterly),
            dec_str(combined.half_yearly),
            dec_str(combined.yearly),
        ],
    );
    y - line * 1.5
}

fn pdf_row(layer: &PdfLayerReference, font: &IndirectFontRef, size: f32, x: Mm, y: Mm, cells: &[String]) {
    let widths = [Mm(26.0), Mm(36.0), Mm(24.0), Mm(26.0), Mm(28.0), Mm(24.0), Mm(26.0)];
    let mut cx = x;
    for (i, cell) in cells.iter().enumerate() {
        if !cell.is_empty() {
            layer.use_text(cell, size, cx, y, font);
        }
        cx = cx + widths.get(i).copied().unwrap_or(Mm(24.0));
    }
}

fn provider_label(p: CloudProvider) -> String {
    p.label().to_string()
}

fn dec_str(d: Decimal) -> String {
    format!("{:.2}", d)
}

fn dec_f64(d: Decimal) -> f64 {
    use rust_decimal::prelude::*;
    d.to_f64().unwrap_or(0.0)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{CostTable, ProviderUiEstimate};
    use chrono::Utc;
    use rust_decimal::Decimal;
    use uuid::Uuid;

    fn sample() -> UiEstimateResponse {
        let costs = PeriodBreakdown::from_monthly(Decimal::new(3037, 2));
        UiEstimateResponse {
            id: Uuid::new_v4(),
            name: "test".into(),
            live_pricing: false,
            providers: vec![ProviderUiEstimate {
                provider: CloudProvider::Aws,
                infrastructure: CostTable {
                    rows: vec![CostRow {
                        category: "Compute".into(),
                        service: "EC2".into(),
                        description: "ec2".into(),
                        unit_price: Decimal::new(416, 4),
                        quantity: Decimal::from(730u32),
                        unit: "hours".into(),
                        usage_display: "730 h".into(),
                        costs: costs.clone(),
                    }],
                    totals: costs.clone(),
                },
                tokens: CostTable {
                    rows: vec![],
                    totals: PeriodBreakdown::zero(),
                },
                combined: costs,
            }],
            created_at: Utc::now(),
        }
    }

    #[test]
    fn all_export_formats() {
        let est = sample();
        for fmt in [ExportFormat::Csv, ExportFormat::Xlsx, ExportFormat::Pdf] {
            let f = export(&ExportRequest {
                format: fmt,
                estimate: est.clone(),
            })
            .unwrap();
            assert!(!f.bytes.is_empty());
        }
    }
}
