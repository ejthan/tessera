// SPDX-License-Identifier: GPL-3.0-or-later

//! `cargo xtask nf3`: measures what a real import adds to the memory of a process.
//!
//! There is no default URL on purpose, as for `seed`. See `docs/building.md`.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail, ensure};
use secrecy::SecretString;
use serde_json::{Value, json};
use tessera_client::{Client, ClientOptions, DirtyValues, Error, ImportAction, ImportParams};
use tessera_core::log::{Filter, LogEntry, RequestLog};
use tessera_core::transfer::import::{ImportJob, ImportProgress, run_import};
use tessera_core::transfer::source::{Format, ReadOptions, Source};
use tessera_core::transfer::{JobControl, JobEnd};
use tokio::sync::watch;
use uuid::Uuid;

const COLLECTION: &str = "tessera_nf3";
const CANCEL_COLLECTION: &str = "tessera_nf3_cancel";
const CANCEL_DOCS: u64 = 200_000;
const BATCH_SIZE: u32 = 1000;
const PAYLOAD_BYTES: usize = 950;
const SAMPLE_EVERY: Duration = Duration::from_millis(200);

/// Parsed command line of `nf3`.
#[derive(Debug, PartialEq)]
pub struct Options {
    url: String,
    key: String,
    docs: u64,
    format: Format,
    app_baseline_mb: f64,
    limit_mb: f64,
    keep_file: Option<PathBuf>,
}

impl Options {
    /// Parses the arguments after `nf3`; `--url` is required.
    pub fn parse(args: &[String]) -> Result<Self> {
        let mut options = Self {
            url: String::new(),
            key: "xyz".to_string(),
            docs: 1_000_000,
            format: Format::Jsonl,
            app_baseline_mb: 0.0,
            limit_mb: 300.0,
            keep_file: None,
        };
        let mut it = args.iter();
        while let Some(arg) = it.next() {
            let mut value = || it.next().with_context(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--url" => options.url = value()?.trim_end_matches('/').to_string(),
                "--key" => options.key = value()?.clone(),
                "--docs" => options.docs = value()?.parse().context("--docs")?,
                "--format" => {
                    options.format = match value()?.as_str() {
                        "jsonl" => Format::Jsonl,
                        "json" => Format::JsonArray,
                        "csv" => Format::Csv,
                        other => bail!("unknown format {other}"),
                    }
                }
                "--app-baseline-mb" => {
                    options.app_baseline_mb = value()?.parse().context("--app-baseline-mb")?;
                }
                "--limit-mb" => options.limit_mb = value()?.parse().context("--limit-mb")?,
                "--keep-file" => options.keep_file = Some(PathBuf::from(value()?)),
                other => bail!("unknown argument {other}"),
            }
        }
        ensure!(
            !options.url.is_empty(),
            "--url is required (there is no default)"
        );
        Ok(options)
    }
}

fn format_name(format: Format) -> &'static str {
    match format {
        Format::Jsonl => "jsonl",
        Format::JsonArray => "json",
        Format::Csv => "csv",
    }
}

fn document(i: u64, payload: &str) -> String {
    format!("{{\"id\":\"{i}\",\"title\":\"doc {i}\",\"n\":{i},\"payload\":\"{payload}\"}}")
}

/// Writes `docs` documents to `path` in `format`.
fn write_documents(path: &Path, format: Format, docs: u64) -> Result<()> {
    let file = File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let payload = "x".repeat(PAYLOAD_BYTES);
    match format {
        Format::Jsonl => {
            for i in 0..docs {
                writeln!(out, "{}", document(i, &payload))?;
            }
        }
        Format::JsonArray => {
            write!(out, "[")?;
            for i in 0..docs {
                let separator = if i == 0 { "" } else { "," };
                write!(out, "{separator}{}", document(i, &payload))?;
            }
            write!(out, "]")?;
        }
        Format::Csv => {
            writeln!(out, "id,title,n,payload")?;
            for i in 0..docs {
                writeln!(out, "{i},doc {i},{i},{payload}")?;
            }
        }
    }
    out.flush()?;
    Ok(())
}

fn read_options(format: Format) -> ReadOptions {
    let mut options = ReadOptions::new(format);
    options.fields = [("title", "string"), ("n", "int64"), ("payload", "string")]
        .map(|(name, ty)| (name.to_string(), ty.to_string()))
        .to_vec();
    options
}

fn schema(name: &str) -> Value {
    json!({
        "name": name,
        "fields": [
            {"name": "title", "type": "string"},
            {"name": "n", "type": "int64"},
            {"name": "payload", "type": "string", "index": false, "optional": true},
        ],
    })
}

fn rss_kib(pid: &str) -> Result<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", pid])
        .output()
        .context("running ps")?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.trim()
        .parse()
        .with_context(|| format!("ps said {text:?}"))
}

/// Keeps the largest RSS of this process until stopped.
struct Sampler {
    stop: Arc<AtomicBool>,
    peak: Arc<AtomicU64>,
    thread: JoinHandle<()>,
}

impl Sampler {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let peak = Arc::new(AtomicU64::new(0));
        let (stopped, highest) = (stop.clone(), peak.clone());
        let thread = std::thread::spawn(move || {
            let pid = std::process::id().to_string();
            while !stopped.load(Ordering::Relaxed) {
                if let Ok(kib) = rss_kib(&pid) {
                    highest.fetch_max(kib, Ordering::Relaxed);
                }
                std::thread::sleep(SAMPLE_EVERY);
            }
        });
        Self { stop, peak, thread }
    }

    /// The peak in KiB.
    fn finish(self) -> u64 {
        self.stop.store(true, Ordering::Relaxed);
        // A panicked sampler leaves the peak it had.
        let _ = self.thread.join();
        self.peak.load(Ordering::Relaxed)
    }
}

/// What the app's total comes to, and whether it stays under the limit.
fn verdict(
    peak_mb: f64,
    baseline_mb: f64,
    app_baseline_mb: f64,
    limit_mb: f64,
) -> (f64, f64, bool) {
    let engine = peak_mb - baseline_mb;
    let total = engine + app_baseline_mb;
    (engine, total, total < limit_mb)
}

fn import_job(source: &Path, format: Format, collection: &str) -> ImportJob {
    ImportJob {
        source: Source::File(source.to_path_buf()),
        options: read_options(format),
        collection: collection.to_string(),
        params: ImportParams {
            action: ImportAction::Create,
            batch_size: BATCH_SIZE,
            dirty_values: DirtyValues::CoerceOrReject,
        },
    }
}

fn import_events(log: &Mutex<RequestLog>) -> Vec<(SystemTime, Duration)> {
    let log = log.lock().unwrap_or_else(|e| e.into_inner());
    log.entries(&Filter::default())
        .into_iter()
        .filter(|entry| entry.event.path.ends_with("documents/import"))
        .map(|entry| (entry.event.started_at, entry.event.duration))
        .collect()
}

async fn ensure_absent(client: &Client, name: &str) -> Result<()> {
    match client.collection(name).await {
        Err(Error::Http { status: 404, .. }) => Ok(()),
        Ok(_) => bail!("a collection named {name} exists; remove it first"),
        Err(e) => Err(e).context("reading the collections"),
    }
}

/// The full run, the import and its check.
async fn import_run(client: &Client, file: &Path, options: &Options) -> Result<(f64, u64)> {
    client.create_collection(&schema(COLLECTION)).await?;
    let sampler = Sampler::start();
    let started = Instant::now();
    let (tx, _rx) = watch::channel(ImportProgress::default());
    let job = import_job(file, options.format, COLLECTION);
    let outcome = run_import(client.clone(), job, JobControl::default(), tx).await;
    let seconds = started.elapsed().as_secs_f64();
    let peak = sampler.finish();
    let p = &outcome.progress;
    ensure!(
        matches!(outcome.end, JobEnd::Finished),
        "import ended {:?}",
        outcome.end
    );
    ensure!(
        p.imported == options.docs && p.failed == 0 && p.unconfirmed == 0,
        "imported {}, failed {}, unconfirmed {} of {}: {:?}",
        p.imported,
        p.failed,
        p.unconfirmed,
        options.docs,
        p.first_error
    );
    ensure!(
        outcome.count.as_ref().is_some_and(|count| count.ok),
        "count check: {:?}",
        outcome.count
    );
    let stored = client.collection(COLLECTION).await?.num_documents;
    ensure!(
        stored == options.docs,
        "the server holds {stored} documents"
    );
    Ok((seconds, peak))
}

/// Stops half way; returns the stop latency in milliseconds.
async fn cancel_run(
    client: &Client,
    file: &Path,
    options: &Options,
    log: &Mutex<RequestLog>,
) -> Result<u128> {
    client.create_collection(&schema(CANCEL_COLLECTION)).await?;
    let docs = options.docs.min(CANCEL_DOCS);
    let control = JobControl::default();
    let (tx, mut rx) = watch::channel(ImportProgress::default());
    let job = import_job(file, options.format, CANCEL_COLLECTION);
    let running = tokio::spawn(run_import(client.clone(), job, control.clone(), tx));
    rx.wait_for(|p| p.imported >= docs / 2).await?;
    // Half a round trip on, so the stop lands in a batch in flight. Right after a progress
    // update it would race the next batch, whose `started_at` follows the check of `stop`
    // by the copy of its body, about 100 µs.
    let round_trip = import_events(log).last().map_or(Duration::ZERO, |e| e.1);
    tokio::time::sleep(round_trip / 2).await;
    let (stop_time, stop) = (SystemTime::now(), Instant::now());
    control.stop.cancel();
    let outcome = running.await?;
    let latency = stop.elapsed().as_millis();
    ensure!(
        matches!(outcome.end, JobEnd::Stopped),
        "cancel run ended {:?}",
        outcome.end
    );
    let late = import_events(log)
        .iter()
        .filter(|(started, _)| *started > stop_time)
        .count();
    ensure!(late == 0, "{late} imports started after the stop");
    Ok(latency)
}

async fn measure(
    client: &Client,
    options: &Options,
    log: &Mutex<RequestLog>,
    dir: &Path,
) -> Result<Value> {
    let keep = options.keep_file.clone();
    let file = keep.unwrap_or_else(|| dir.join("nf3.data"));
    write_documents(&file, options.format, options.docs)?;
    let cancel_file = dir.join("nf3-cancel.data");
    write_documents(&cancel_file, options.format, options.docs.min(CANCEL_DOCS))?;

    let baseline = rss_kib(&std::process::id().to_string())?;
    let (seconds, peak) = import_run(client, &file, options).await?;
    let slowest = import_events(log)
        .iter()
        .map(|(_, duration)| duration.as_millis())
        .max()
        .unwrap_or(0);
    let stop_latency = cancel_run(client, &cancel_file, options, log).await?;

    let (peak_mb, baseline_mb) = (peak as f64 / 1024.0, baseline as f64 / 1024.0);
    let (engine, total, pass) = verdict(
        peak_mb,
        baseline_mb,
        options.app_baseline_mb,
        options.limit_mb,
    );
    Ok(json!({
        "docs": options.docs,
        "format": format_name(options.format),
        "seconds": seconds,
        "docs_per_second": options.docs as f64 / seconds,
        "peak_rss_mb": peak_mb,
        "baseline_rss_mb": baseline_mb,
        "engine_rss_mb": engine,
        "app_baseline_mb": options.app_baseline_mb,
        "total_rss_mb": total,
        "limit_mb": options.limit_mb,
        "slowest_batch_ms": slowest,
        "stop_latency_ms": stop_latency,
        "pass": pass,
    }))
}

/// Runs the measurement and prints its line; `Ok(false)` when the limit is exceeded.
pub fn run(options: &Options) -> Result<bool> {
    // As the app: see `crates/tessera-app/src/lib.rs`.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let log = Arc::new(Mutex::new(RequestLog::default()));
        let sink = log.clone();
        let client = Client::new(
            &[options.url.as_str()],
            SecretString::from(options.key.clone()),
            ClientOptions::default(),
        )?
        .with_hook(Arc::new(move |event| {
            let entry = LogEntry {
                connection: Uuid::nil(),
                connection_name: Arc::from("nf3"),
                event: event.clone(),
            };
            sink.lock().unwrap_or_else(|e| e.into_inner()).push(entry);
        }));
        ensure_absent(&client, COLLECTION).await?;
        ensure_absent(&client, CANCEL_COLLECTION).await?;
        let dir = tempfile::tempdir()?;
        let measured = measure(&client, options, &log, dir.path()).await;
        for name in [COLLECTION, CANCEL_COLLECTION] {
            // One that was never created answers 404.
            let _ = client.delete_collection(name).await;
        }
        let line = measured?;
        println!("{line}");
        Ok(line["pass"].as_bool().unwrap_or(false))
    })
}

#[cfg(test)]
mod tests {
    // Tests unwrap; a panic is the failure we want.
    #![allow(clippy::unwrap_used)]

    use std::ops::ControlFlow;

    use tessera_core::transfer::source::read_documents;

    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn options_parse() {
        let options = Options::parse(&args(&[
            "--url",
            "http://localhost:18108/",
            "--docs",
            "10",
            "--format",
            "csv",
            "--app-baseline-mb",
            "90",
            "--keep-file",
            "/tmp/x",
        ]))
        .unwrap();
        assert_eq!(options.url, "http://localhost:18108");
        assert_eq!(options.key, "xyz");
        assert_eq!(options.docs, 10);
        assert_eq!(options.format, Format::Csv);
        assert_eq!(options.app_baseline_mb, 90.0);
        assert_eq!(options.limit_mb, 300.0);
        assert_eq!(options.keep_file, Some(PathBuf::from("/tmp/x")));
        assert!(Options::parse(&args(&["--url", "u", "--format", "xml"])).is_err());
        assert!(Options::parse(&args(&["--url"])).is_err());
    }

    #[test]
    fn missing_url_is_an_error() {
        assert!(Options::parse(&args(&["--docs", "1"])).is_err());
    }

    #[test]
    fn document_is_about_one_kilobyte() {
        let length = document(999_999, &"x".repeat(PAYLOAD_BYTES)).len();
        assert!((1000..=1040).contains(&length), "{length}");
    }

    #[test]
    fn json_and_csv_writers_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        for format in [Format::Jsonl, Format::JsonArray, Format::Csv] {
            let path = dir.path().join("docs");
            write_documents(&path, format, 3).unwrap();
            let mut docs = Vec::new();
            read_documents(&Source::File(path), &read_options(format), |record| {
                let text = record.doc.unwrap();
                docs.push(serde_json::from_str::<Value>(&text).unwrap());
                ControlFlow::Continue(())
            })
            .unwrap();
            assert_eq!(docs.len(), 3, "{format:?}");
            assert_eq!(docs[2]["id"], "2", "{format:?}");
            assert_eq!(docs[2]["title"], "doc 2", "{format:?}");
            assert_eq!(docs[2]["n"], 2, "{format:?}");
            assert_eq!(
                docs[2]["payload"].as_str().unwrap().len(),
                PAYLOAD_BYTES,
                "{format:?}"
            );
        }
    }

    #[test]
    fn verdict_adds_the_baseline() {
        assert_eq!(verdict(120.0, 20.0, 150.0, 300.0), (100.0, 250.0, true));
        assert_eq!(verdict(170.0, 20.0, 150.0, 300.0), (150.0, 300.0, false));
    }
}
