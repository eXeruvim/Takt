#[allow(dead_code)]
mod player {
    pub mod limits;
    pub mod playlist;
}

use std::{
    env,
    ffi::OsString,
    fs,
    hint::black_box,
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use player::playlist::{
    benchmark_playlist_labels, benchmark_scan_media_files, benchmark_sort_paths,
};
use serde_json::json;
use slint::{ModelRc, SharedString, VecModel};

struct Arguments {
    directory: PathBuf,
    output: PathBuf,
    workload_sha256: String,
    expected_files: usize,
    iterations: usize,
    warmup_iterations: usize,
}

#[derive(Default)]
struct Samples {
    scan_ms: Vec<f64>,
    sort_ms: Vec<f64>,
    labels_ms: Vec<f64>,
    shared_strings_ms: Vec<f64>,
    model_ms: Vec<f64>,
    total_ms: Vec<f64>,
}

struct Sample {
    scan_ms: f64,
    sort_ms: f64,
    labels_ms: f64,
    shared_strings_ms: f64,
    model_ms: f64,
    total_ms: f64,
    kept_files: usize,
    omitted_files: usize,
}

fn build_info() -> serde_json::Value {
    json!({
        "schema": 1,
        "package_version": env!("CARGO_PKG_VERSION"),
        "git_commit": env!("OXIDE_BUILD_GIT_COMMIT"),
        "tracked_clean": env!("OXIDE_BUILD_TRACKED_CLEAN") == "true",
        "tracked_state": env!("OXIDE_BUILD_TRACKED_CLEAN"),
    })
}

fn take_value(arguments: &mut impl Iterator<Item = OsString>, name: &str) -> Result<OsString> {
    arguments
        .next()
        .with_context(|| format!("Для {name} не указано значение"))
}

fn parse_usize(value: OsString, name: &str, minimum: usize, maximum: usize) -> Result<usize> {
    let value = value
        .to_str()
        .with_context(|| format!("{name} должен быть текстовым числом"))?
        .parse::<usize>()
        .with_context(|| format!("{name} должен быть целым числом"))?;
    if !(minimum..=maximum).contains(&value) {
        bail!("{name} должен быть в диапазоне {minimum}..={maximum}");
    }
    Ok(value)
}

fn parse_arguments() -> Result<Arguments> {
    let mut directory = None;
    let mut output = None;
    let mut workload_sha256 = None;
    let mut expected_files = None;
    let mut iterations = 25;
    let mut warmup_iterations = 3;
    let mut arguments = env::args_os().skip(1);

    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--directory") => {
                directory = Some(PathBuf::from(take_value(&mut arguments, "--directory")?));
            }
            Some("--output") => {
                output = Some(PathBuf::from(take_value(&mut arguments, "--output")?));
            }
            Some("--workload-sha256") => {
                workload_sha256 = Some(
                    take_value(&mut arguments, "--workload-sha256")?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Some("--expected-files") => {
                expected_files = Some(parse_usize(
                    take_value(&mut arguments, "--expected-files")?,
                    "--expected-files",
                    1,
                    100_000,
                )?);
            }
            Some("--iterations") => {
                iterations = parse_usize(
                    take_value(&mut arguments, "--iterations")?,
                    "--iterations",
                    3,
                    1_000,
                )?;
            }
            Some("--warmup-iterations") => {
                warmup_iterations = parse_usize(
                    take_value(&mut arguments, "--warmup-iterations")?,
                    "--warmup-iterations",
                    1,
                    100,
                )?;
            }
            Some(other) => bail!("Неизвестный аргумент: {other}"),
            None => bail!("Аргументы должны быть корректным Unicode"),
        }
    }

    Ok(Arguments {
        directory: directory.context("Не указан --directory")?,
        output: output.context("Не указан --output")?,
        workload_sha256: workload_sha256.context("Не указан --workload-sha256")?,
        expected_files: expected_files.context("Не указан --expected-files")?,
        iterations,
        warmup_iterations,
    })
}

fn elapsed_ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000.0
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len().saturating_sub(1));
    sorted[index]
}

fn statistics(values: &[f64]) -> serde_json::Value {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    json!({
        "min": sorted[0],
        "p50": percentile(&sorted, 0.50),
        "p95": percentile(&sorted, 0.95),
        "max": sorted[sorted.len() - 1],
        "values": values,
    })
}

fn run_sample(directory: &Path) -> Result<Sample> {
    let total_started = Instant::now();

    let scan_started = Instant::now();
    let (mut files, omitted_count) = benchmark_scan_media_files(directory)?;
    let scan_ms = elapsed_ms(scan_started);

    let sort_started = Instant::now();
    benchmark_sort_paths(black_box(&mut files));
    let sort_ms = elapsed_ms(sort_started);

    let labels_started = Instant::now();
    let labels = benchmark_playlist_labels(black_box(&files));
    let labels_ms = elapsed_ms(labels_started);

    let shared_strings_started = Instant::now();
    let shared_strings = labels
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    let shared_strings_ms = elapsed_ms(shared_strings_started);

    let model_started = Instant::now();
    let model: ModelRc<SharedString> = ModelRc::new(VecModel::from(shared_strings));
    black_box(&model);
    let model_ms = elapsed_ms(model_started);

    Ok(Sample {
        scan_ms,
        sort_ms,
        labels_ms,
        shared_strings_ms,
        model_ms,
        total_ms: elapsed_ms(total_started),
        kept_files: files.len(),
        omitted_files: omitted_count,
    })
}

fn main() -> Result<()> {
    if env::args_os().nth(1).as_deref() == Some(std::ffi::OsStr::new("--build-info")) {
        println!("{}", build_info());
        return Ok(());
    }

    let arguments = parse_arguments()?;
    if !arguments.directory.is_dir() {
        bail!(
            "Каталог fixture не найден: {}",
            arguments.directory.display()
        );
    }
    if let Some(parent) = arguments.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Не удалось создать каталог: {}", parent.display()))?;
    }

    for _ in 0..arguments.warmup_iterations {
        black_box(run_sample(&arguments.directory)?);
    }

    let mut samples = Samples::default();
    let mut kept_files = 0;
    let mut omitted_files = 0;
    for _ in 0..arguments.iterations {
        let sample = run_sample(&arguments.directory)?;
        kept_files = sample.kept_files;
        omitted_files = sample.omitted_files;
        samples.scan_ms.push(sample.scan_ms);
        samples.sort_ms.push(sample.sort_ms);
        samples.labels_ms.push(sample.labels_ms);
        samples.shared_strings_ms.push(sample.shared_strings_ms);
        samples.model_ms.push(sample.model_ms);
        samples.total_ms.push(sample.total_ms);
    }

    if kept_files.saturating_add(omitted_files) != arguments.expected_files {
        bail!(
            "Fixture изменился во время замера: ожидалось {}, обработано {}",
            arguments.expected_files,
            kept_files.saturating_add(omitted_files)
        );
    }

    let created_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let report = json!({
        "schema": 1,
        "kind": "oxide-playlist-phases",
        "created_unix_ms": created_unix_ms,
        "build": build_info(),
        "workload": {
            "sha256": arguments.workload_sha256,
            "expected_files": arguments.expected_files,
            "kept_files": kept_files,
            "omitted_files": omitted_files,
        },
        "iterations": arguments.iterations,
        "warmup_iterations": arguments.warmup_iterations,
        "metrics_ms": {
            "directory_scan": statistics(&samples.scan_ms),
            "natural_sort": statistics(&samples.sort_ms),
            "ui_labels": statistics(&samples.labels_ms),
            "shared_strings": statistics(&samples.shared_strings_ms),
            "slint_vec_model": statistics(&samples.model_ms),
            "total": statistics(&samples.total_ms),
        },
    });
    fs::write(&arguments.output, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("Не удалось записать отчёт: {}", arguments.output.display()))?;
    println!("Готово: {}", arguments.output.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_uses_nearest_rank_without_hiding_the_slowest_sample() {
        let values = [1.0, 2.0, 3.0, 4.0, 50.0];
        assert_eq!(percentile(&values, 0.50), 3.0);
        assert_eq!(percentile(&values, 0.95), 50.0);
        assert_eq!(percentile(&values, 1.00), 50.0);
    }
}
