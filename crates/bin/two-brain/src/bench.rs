//! Read-only workspace benchmark & performance profiling subcommand (`2brain bench`).
//!
//! Measures cold load, single-file incremental refresh (without mutating disk),
//! BM25 search, cognitive topology/gaps/contradictions/fragility, Git diff graph,
//! and resident memory footprint (`VmRSS`/`VmHWM` on Linux).

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tb_app::App;

#[derive(Debug, Clone)]
struct TimingStats {
    median_ms: f64,
    min_ms: f64,
    max_ms: f64,
}

fn measure_n<F: FnMut()>(iterations: usize, mut f: F) -> TimingStats {
    let n = iterations.max(1);
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        f();
        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.total_cmp(b));
    let median_ms = samples[samples.len() / 2];
    let min_ms = samples[0];
    let max_ms = samples[samples.len() - 1];
    TimingStats {
        median_ms,
        min_ms,
        max_ms,
    }
}

#[cfg(target_os = "linux")]
fn read_proc_memory_kib() -> (Option<u64>, Option<u64>) {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let mut rss = None;
    let mut hwm = None;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            rss = rest.split_whitespace().next().and_then(|s| s.parse().ok());
        } else if let Some(rest) = line.strip_prefix("VmHWM:") {
            hwm = rest.split_whitespace().next().and_then(|s| s.parse().ok());
        }
    }
    (rss, hwm)
}

#[cfg(not(target_os = "linux"))]
fn read_proc_memory_kib() -> (Option<u64>, Option<u64>) {
    (None, None)
}

pub fn run_bench(path: &Path, iterations: usize, as_json: bool) {
    let iters = iterations.clamp(1, 50);
    let canonical = match std::fs::canonicalize(path) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error resolving workspace '{}': {err}", path.display());
            std::process::exit(1);
        }
    };

    // 1. Cold full load
    let cold_t0 = Instant::now();
    let (store, snap) = match tb_fs_git::load_snapshot(&canonical) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error loading workspace '{}': {err}", canonical.display());
            std::process::exit(1);
        }
    };
    let cold_load_ms = cold_t0.elapsed().as_secs_f64() * 1000.0;

    let file_count = snap.files.len();
    let node_count = snap.graph.node_count();
    let edge_count = snap.graph.edge_count();
    let diag_count = snap.diagnostics.len();

    // Pick a real file path for non-mutating single-file incremental refresh probe.
    let probe_abs = snap.files.keys().next().and_then(|loc| {
        store
            .member_root(&loc.repo)
            .map(|root| root.join(&loc.path))
    });

    // Pick a representative query term from the first non-empty file title or path.
    let sample_query = snap
        .files
        .values()
        .find_map(|f| {
            f.body()
                .split(|c: char| !c.is_alphanumeric())
                .find(|w| w.len() >= 5 && w.is_ascii())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "index".to_owned());

    let app = Arc::new(App::new(store, snap));
    let type_count = app
        .types(None)
        .get("types")
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let base_snap = app.snapshot();

    // 2. Single-file incremental refresh (read-only: re-reads & re-indexes one existing file)
    let incremental_stats = probe_abs.as_ref().map(|abs| {
        let mut gen_cursor = base_snap.generation;
        measure_n(iters, || {
            gen_cursor = gen_cursor.next();
            let _ = tb_fs_git::refresh_snapshot_incremental(
                &canonical,
                &base_snap,
                std::slice::from_ref(abs),
                gen_cursor,
            );
        })
    });

    // 3. BM25 search
    let search_stats = measure_n(iters.saturating_mul(4).min(40), || {
        let _ = app.search(&sample_query, None, false, None, 0, 10);
    });

    // 4. Cognitive suite (cold uncached + warm cached orient)
    let empty_suppressed = HashSet::new();
    let orient_cold_t0 = Instant::now();
    let _ = app.orient(&empty_suppressed);
    let orient_cold_ms = orient_cold_t0.elapsed().as_secs_f64() * 1000.0;

    let orient_cached_stats = measure_n(iters.saturating_mul(4).min(40), || {
        let _ = app.orient(&empty_suppressed);
    });

    let topology_t0 = Instant::now();
    let _ = app.analyze_topology(5, 10);
    let topology_ms = topology_t0.elapsed().as_secs_f64() * 1000.0;

    let gaps_t0 = Instant::now();
    let _ = app.detect_gaps(&empty_suppressed, 10);
    let gaps_ms = gaps_t0.elapsed().as_secs_f64() * 1000.0;

    let contradictions_t0 = Instant::now();
    let _ = app.audit_contradictions(None, 25);
    let contradictions_ms = contradictions_t0.elapsed().as_secs_f64() * 1000.0;

    let fragility_t0 = Instant::now();
    let _ = app.fragility_report(15);
    let fragility_ms = fragility_t0.elapsed().as_secs_f64() * 1000.0;

    // 5. Optional Git diff graph (`HEAD~5` or `HEAD~1`)
    let after_meta = tb_app::snapshot_node_meta(&base_snap);
    let diff_ms = ["HEAD~5", "HEAD~1"].into_iter().find_map(|git_ref| {
        let t0 = Instant::now();
        let old = tb_fs_git::load_snapshot_at_git_ref(&canonical, git_ref).ok()?;
        let before_meta = tb_app::snapshot_node_meta(&old);
        let _ = tb_cognition::diff_snapshots(
            &old.graph,
            &before_meta,
            &base_snap.graph,
            &after_meta,
            10,
        );
        Some((git_ref, t0.elapsed().as_secs_f64() * 1000.0))
    });

    let (rss_kib, hwm_kib) = read_proc_memory_kib();

    if as_json {
        let report = json!({
            "version": env!("CARGO_PKG_VERSION"),
            "workspace": canonical.display().to_string(),
            "iterations": iters,
            "counts": {
                "files": file_count,
                "nodes": node_count,
                "edges": edge_count,
                "types": type_count,
                "diagnostics": diag_count,
            },
            "timings_ms": {
                "cold_load": cold_load_ms,
                "incremental_reload_median": incremental_stats.as_ref().map(|s| s.median_ms),
                "incremental_reload_min": incremental_stats.as_ref().map(|s| s.min_ms),
                "incremental_reload_max": incremental_stats.as_ref().map(|s| s.max_ms),
                "search_median": search_stats.median_ms,
                "orient_cold": orient_cold_ms,
                "orient_cached_median": orient_cached_stats.median_ms,
                "analyze_topology": topology_ms,
                "detect_gaps": gaps_ms,
                "audit_contradictions": contradictions_ms,
                "fragility_report": fragility_ms,
                "diff_graph": diff_ms.map(|(r, ms)| json!({ "from_ref": r, "ms": ms })),
            },
            "memory_kib": {
                "vm_rss": rss_kib,
                "vm_hwm": hwm_kib,
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).expect("json serialization")
        );
        return;
    }

    println!(
        "2brain bench {} (files={}, nodes={}, edges={}, types={}, diagnostics={})",
        env!("CARGO_PKG_VERSION"),
        file_count,
        node_count,
        edge_count,
        type_count,
        diag_count
    );
    println!("  cold_load            {:>8.2} ms", cold_load_ms);
    if let Some(inc) = incremental_stats {
        let speedup = if inc.median_ms > 0.0 {
            cold_load_ms / inc.median_ms
        } else {
            0.0
        };
        println!(
            "  incremental_reload   {:>8.2} ms  (min={:.2}, max={:.2}, {:.1}x vs cold)",
            inc.median_ms, inc.min_ms, inc.max_ms, speedup
        );
    }
    println!(
        "  bm25_search          {:>8.2} ms  (query='{}', min={:.2}, max={:.2})",
        search_stats.median_ms, sample_query, search_stats.min_ms, search_stats.max_ms
    );
    println!(
        "  orient (cold/cached) {:>8.2} ms / {:.3} ms",
        orient_cold_ms, orient_cached_stats.median_ms
    );
    println!("  analyze_topology     {:>8.2} ms", topology_ms);
    println!("  detect_gaps          {:>8.2} ms", gaps_ms);
    println!("  audit_contradictions {:>8.2} ms", contradictions_ms);
    println!("  fragility_report     {:>8.2} ms", fragility_ms);
    if let Some((git_ref, ms)) = diff_ms {
        println!("  diff_graph ({git_ref})  {:>8.2} ms", ms);
    }
    if let (Some(rss), Some(hwm)) = (rss_kib, hwm_kib) {
        println!(
            "  memory               VmRSS={:.1} MiB, VmHWM={:.1} MiB",
            rss as f64 / 1024.0,
            hwm as f64 / 1024.0
        );
    }
}
