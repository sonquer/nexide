//! Render benchmark results as terminal tables. Two tables: absolute
//! metrics (one row per `route × runtime` cell) and a delta table
//! comparing the first listed runtime against the rest.

use std::collections::BTreeMap;
use std::time::Duration;

use comfy_table::{Cell, ContentArrangement, Table, presets::UTF8_FULL};

use crate::runner::BenchResult;
use crate::target::TargetKind;

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn delta(new: f64, base: f64) -> String {
    if base.abs() < f64::EPSILON {
        return "n/a".into();
    }
    let pct = (new - base) / base * 100.0;
    let sign = if pct >= 0.0 { '+' } else { '-' };
    format!("{sign}{:>6.1}%", pct.abs())
}

fn absolute_table(results: &[BenchResult]) -> Table {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "route",
            "runtime",
            "RPS",
            "p50 ms",
            "p95 ms",
            "p99 ms",
            "cpu avg %",
            "cpu max %",
            "mem avg MB",
            "mem max MB",
            "threads",
            "RPS/CPU%",
            "errors",
            "transport",
            "http5xx",
            "timeout",
        ]);
    for r in results {
        let rps_per_cpu = if r.sample.cpu_avg > 0.0 {
            r.load.rps / r.sample.cpu_avg * 100.0
        } else {
            0.0
        };
        table.add_row(vec![
            Cell::new(&r.route),
            Cell::new(r.runtime.label()),
            Cell::new(format!("{:>8.0}", r.load.rps)),
            Cell::new(format!("{:>6.2}", ms(r.load.p50))),
            Cell::new(format!("{:>6.2}", ms(r.load.p95))),
            Cell::new(format!("{:>6.2}", ms(r.load.p99))),
            Cell::new(format!("{:>7.1}", r.sample.cpu_avg)),
            Cell::new(format!("{:>7.1}", r.sample.cpu_max)),
            Cell::new(format!("{:>7.1}", r.sample.mem_avg_mb)),
            Cell::new(format!("{:>7.1}", r.sample.mem_max_mb)),
            Cell::new(r.sample.threads_max),
            Cell::new(format!("{rps_per_cpu:>8.1}")),
            Cell::new(r.load.errors),
            Cell::new(r.load.transport_errors),
            Cell::new(r.load.http_errors),
            Cell::new(r.load.timeout_errors),
        ]);
    }
    table
}

fn delta_table(results: &[BenchResult], protagonist: TargetKind) -> Option<Table> {
    let mut by_route: BTreeMap<String, BTreeMap<TargetKind, &BenchResult>> = BTreeMap::new();
    for r in results {
        by_route
            .entry(r.route.clone())
            .or_default()
            .insert(r.runtime, r);
    }
    let mut baselines: Vec<TargetKind> = results
        .iter()
        .map(|r| r.runtime)
        .filter(|r| *r != protagonist)
        .collect();
    baselines.sort();
    baselines.dedup();
    if baselines.is_empty() {
        return None;
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec![
            "route", "vs", "Δ RPS", "Δ p50", "Δ p95", "Δ p99", "Δ cpu", "Δ mem",
        ]);
    for (route, cells) in &by_route {
        let Some(new) = cells.get(&protagonist) else {
            continue;
        };
        for baseline in &baselines {
            let Some(base) = cells.get(baseline) else {
                continue;
            };
            table.add_row(vec![
                Cell::new(route),
                Cell::new(baseline.label()),
                Cell::new(delta(new.load.rps, base.load.rps)),
                Cell::new(delta(ms(new.load.p50), ms(base.load.p50))),
                Cell::new(delta(ms(new.load.p95), ms(base.load.p95))),
                Cell::new(delta(ms(new.load.p99), ms(base.load.p99))),
                Cell::new(delta(new.sample.cpu_avg, base.sample.cpu_avg)),
                Cell::new(delta(new.sample.mem_avg_mb, base.sample.mem_avg_mb)),
            ]);
        }
    }
    Some(table)
}

/// Format `results` as two stacked tables (absolute + delta showing
/// `protagonist` against every other runtime present).
#[must_use]
pub fn render_report(results: &[BenchResult], protagonist: TargetKind) -> String {
    let mut out = String::new();
    out.push_str("\n>>> results\n");
    out.push_str(&absolute_table(results).to_string());
    out.push('\n');
    if let Some(delta) = delta_table(results, protagonist) {
        out.push_str(&format!(
            "\n>>> delta vs baselines ({} vs baseline)\n",
            protagonist.label()
        ));
        out.push_str(&delta.to_string());
        out.push('\n');
    }
    out
}

/// Single point of a concurrency sweep (one full bench run at a given
/// connection count).
#[derive(Debug, Clone)]
pub struct SweepPoint {
    /// Concurrent virtual users used for this run.
    pub connections: usize,
    /// All `(route, runtime)` cells measured at that concurrency level.
    pub results: Vec<BenchResult>,
}

/// Render an RPS-vs-concurrency scaling matrix: one row per
/// `(route, runtime)` cell, one column per swept concurrency level.
#[must_use]
pub fn render_scaling_rps(points: &[SweepPoint]) -> String {
    render_scaling(points, "RPS", |r| format!("{:>8.0}", r.load.rps))
}

/// Render a p99-vs-concurrency scaling matrix in milliseconds.
#[must_use]
pub fn render_scaling_p99(points: &[SweepPoint]) -> String {
    render_scaling(points, "p99 ms", |r| format!("{:>6.2}", ms(r.load.p99)))
}

/// Render an RSS-vs-concurrency scaling matrix in megabytes.
#[must_use]
pub fn render_scaling_mem(points: &[SweepPoint]) -> String {
    render_scaling(points, "mem avg MB", |r| {
        format!("{:>7.1}", r.sample.mem_avg_mb)
    })
}

fn render_scaling<F>(points: &[SweepPoint], metric: &str, fmt: F) -> String
where
    F: Fn(&BenchResult) -> String,
{
    let mut by_cell: BTreeMap<(String, TargetKind), BTreeMap<usize, String>> = BTreeMap::new();
    let mut conns: Vec<usize> = points.iter().map(|p| p.connections).collect();
    conns.sort_unstable();
    conns.dedup();
    for point in points {
        for r in &point.results {
            by_cell
                .entry((r.route.clone(), r.runtime))
                .or_default()
                .insert(point.connections, fmt(r));
        }
    }
    let mut header: Vec<Cell> = vec![Cell::new("route"), Cell::new("runtime")];
    for c in &conns {
        header.push(Cell::new(format!("c={c}")));
    }
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(header);
    for ((route, runtime), per_conn) in &by_cell {
        let mut row: Vec<Cell> = vec![Cell::new(route), Cell::new(runtime.label())];
        for c in &conns {
            row.push(Cell::new(per_conn.get(c).cloned().unwrap_or_default()));
        }
        table.add_row(row);
    }
    format!("\n>>> scaling - {metric}\n{table}\n")
}
