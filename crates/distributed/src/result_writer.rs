//! 원본 `Distributed/ResultWriter.cs`: Controller CSV·JSON 결과와 집계.
//!
//! - CSV: UTF-8 BOM, 줄바꿈 `Environment.NewLine`, 같은 `SampleId`로 Worker 행들과 전체(`total`) 행을 쓴다.
//!   값은 인베리언트 서식(날짜 `"O"`, 불리언 `true`/`false`, 실수는 가장 짧은 왕복 표현)이고 계산할 수 없는 값은 빈 칸이다.
//! - JSON: 생성자에서 `Preparing`으로 한 번 쓰고, [`ResultWriter::save_final`]이 같은 파일을 처음부터 다시 쓴다.
//! - 생성자가 CSV 만들기에 실패하면 이미 만든 JSON(과 CSV)을 지운다(TESTCore `ec427f2`).
//! - 원본 그대로: 표본 시각이 같거나 과거인 표본은 증분 없이 기준만 갱신하고, 합계 행의 기준은 일부 Worker가 빠진
//!   조회 전후로 초기화한다. 실수 `ToString()`이 지수 표기로 바뀌는 `1E+15` 이상은 다루지 않는다(처리량으로 나올 수 없다).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use testrust_common::dotnet_json::{NEW_LINE, double_text};
use testrust_common::{DotnetDateTime, DotnetDateTimeOffset, to_dotnet_json};
use testrust_s3::s3_client::error::full_path;
use testrust_scenarios::ScenarioError;
use testrust_scenarios::input::io_error as path_error;

use crate::console::{self, FormatOptions};
use crate::contracts::{RunResult, RunSnapshot, WorkloadSettings};
use crate::settings::{DistributedSettings, DriverSettings, argument};

/// 원본 `ResultWriter.Operations`.
pub const OPERATIONS: [&str; 5] = ["Read", "Write", "Head", "Delete", "List"];

/// 원본 `WorkerSample`: 한 번의 Worker 조회 결과. `snapshot`이 `None`이면 통계를 받지 못한 것이다.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerSample {
    pub worker_id: String,
    pub snapshot: Option<RunSnapshot>,
    pub error: Option<String>,
}

impl WorkerSample {
    pub fn new(worker_id: impl Into<String>, snapshot: Option<RunSnapshot>) -> Self {
        Self {
            worker_id: worker_id.into(),
            snapshot,
            error: None,
        }
    }

    pub fn failed(worker_id: impl Into<String>, error: impl Into<String>) -> Self {
        Self {
            worker_id: worker_id.into(),
            snapshot: None,
            error: Some(error.into()),
        }
    }
}

/// 결과 JSON의 `Settings`(원본의 익명 개체).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResultSettings {
    pub workload: Option<WorkloadSettings>,
    pub drivers: Vec<DriverJson>,
    pub poll_interval_seconds: i32,
    pub start_delay_seconds: i32,
    pub lease_timeout_seconds: i32,
}

/// `DriverSettings(Name, Url)`의 JSON 형태.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct DriverJson {
    pub name: String,
    pub url: String,
}

impl From<&DriverSettings> for DriverJson {
    fn from(d: &DriverSettings) -> Self {
        Self {
            name: d.name.clone(),
            url: d.url.clone(),
        }
    }
}

impl ResultSettings {
    pub fn new(workload: Option<WorkloadSettings>, settings: &DistributedSettings) -> Self {
        Self {
            workload,
            drivers: settings.drivers.iter().map(DriverJson::from).collect(),
            poll_interval_seconds: settings.poll_interval_seconds,
            start_delay_seconds: settings.start_delay_seconds,
            lease_timeout_seconds: settings.lease_timeout_seconds,
        }
    }
}

/// 원본 `new { RunId, ... }` 보고서(속성 순서가 파일 순서다).
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct Report<'a> {
    run_id: &'a str,
    test_type: &'a str,
    state: &'a str,
    error: Option<&'a str>,
    settings: Option<&'a ResultSettings>,
    expected_workers: usize,
    reported_workers: usize,
    unavailable_workers: Vec<&'a str>,
    complete: bool,
    start_skew_milliseconds: Option<f64>,
    success_ops_per_second: Option<f64>,
    total: &'a RunSnapshot,
    workers: Vec<&'a RunSnapshot>,
}

/// 삽입 순서를 지키는 `Dictionary`(원본은 지우지 않으므로 열거 순서 = 삽입 순서).
struct Ordered<T>(Vec<(String, T)>);

impl<T> Ordered<T> {
    fn set(&mut self, key: &str, value: T) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_string(), value)),
        }
    }

    fn values(&self) -> impl Iterator<Item = &T> {
        self.0.iter().map(|(_, v)| v)
    }
}

pub struct ResultWriter {
    csv: File,
    json: File,
    run_id: String,
    test_type: String,
    expected: usize,
    latest: Ordered<RunSnapshot>,
    availability: Ordered<bool>,
    baselines: HashMap<String, (DotnetDateTimeOffset, [i64; 10])>,
    sequence: i64,
    previous_total_available: bool,
    final_logged: bool,
    json_path: PathBuf,
    csv_path: PathBuf,
}

impl ResultWriter {
    /// 원본 생성자: 경로를 정하고 JSON·CSV를 만든다(이미 있으면 `IOException`).
    pub fn new(
        settings: &DistributedSettings,
        save: Option<&str>,
        run_id: &str,
        test_type: &str,
    ) -> Result<Self, ScenarioError> {
        let save = save.filter(|s| !s.trim().is_empty());
        let path = match save {
            None => settings.result_path.clone(),
            Some(save) => settings.resolve_path(save),
        };
        let json_path = match save {
            Some(_) if has_extension(&path) => {
                if !get_extension(&path).eq_ignore_ascii_case(".json") {
                    return Err(argument("--save 파일 경로의 확장자는 .json이어야 합니다."));
                }
                path
            }
            _ => path.join(format!("{run_id}.json")),
        };
        let csv_path = change_extension_csv(&json_path);
        if let Some(dir) = json_path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| io_error(dir, &e))?;
        }
        if json_path.is_file() || csv_path.is_file() {
            return Err(ScenarioError::new(
                "System.IO.IOException",
                "결과 파일이 이미 존재합니다.",
            ));
        }
        let json = create_new(&json_path).map_err(|e| io_error(&json_path, &e))?;
        let csv = match create_new(&csv_path) {
            Ok(csv) => csv,
            Err(e) => {
                drop(json);
                let _ = std::fs::remove_file(&json_path);
                return Err(io_error(&csv_path, &e));
            }
        };
        let mut writer = Self {
            csv,
            json,
            run_id: run_id.to_string(),
            test_type: test_type.to_string(),
            expected: settings.drivers.len(),
            latest: Ordered(Vec::new()),
            availability: Ordered(Vec::new()),
            baselines: HashMap::new(),
            sequence: 0,
            previous_total_available: false,
            final_logged: false,
            json_path,
            csv_path,
        };
        let init = writer
            .write_header()
            .and_then(|()| writer.save_final("Preparing", None, None));
        if let Err(e) = init {
            // 여기서 만든 결과 파일은 지워 빈 파일이 남지 않게 한다.
            let (json_path, csv_path) = (writer.json_path.clone(), writer.csv_path.clone());
            drop(writer);
            let _ = std::fs::remove_file(json_path);
            let _ = std::fs::remove_file(csv_path);
            return Err(e);
        }
        Ok(writer)
    }

    pub fn json_path(&self) -> &Path {
        &self.json_path
    }

    pub fn csv_path(&self) -> &Path {
        &self.csv_path
    }

    fn write_header(&mut self) -> Result<(), ScenarioError> {
        let mut header: Vec<String> = [
            "RunId",
            "SampleId",
            "TestType",
            "Scope",
            "WorkerId",
            "CollectedAtUtc",
            "WorkerSampleAtUtc",
            "ElapsedSeconds",
            "IntervalSeconds",
            "State",
            "Available",
            "IsFinal",
            "Error",
        ]
        .map(String::from)
        .to_vec();
        for op in OPERATIONS {
            header.extend([
                format!("{op}Success"),
                format!("{op}Failed"),
                format!("{op}OpsPerSecond"),
            ]);
        }
        header.extend(
            [
                "EstimatedReadBytesPerSecond",
                "EstimatedWriteBytesPerSecond",
                "ExpectedWorkers",
                "ReportedWorkers",
            ]
            .map(String::from),
        );
        // StreamWriter(UTF8Encoding(true))는 첫 쓰기에서 BOM을 함께 쓴다.
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend(header.join(",").as_bytes());
        bytes.extend(NEW_LINE.as_bytes());
        self.csv.write_all(&bytes)?;
        self.csv.flush()?;
        Ok(())
    }

    /// 같은 SampleId로 Worker 행과 전체 행을 기록한다.
    pub fn sample(
        &mut self,
        samples: &[WorkerSample],
        collected_at: DotnetDateTimeOffset,
    ) -> Result<(), ScenarioError> {
        self.observe(samples);
        self.sequence += 1;
        let reported = samples.iter().filter(|s| s.snapshot.is_some()).count();
        let mut text = String::new();
        for item in samples {
            if let Some(snapshot) = &item.snapshot {
                self.latest.set(&item.worker_id, snapshot.clone());
            }
            self.write_row(
                &mut text,
                &item.worker_id,
                "worker",
                item.snapshot.as_ref(),
                item.error.as_deref(),
                collected_at,
                reported,
                item.snapshot.is_some(),
            );
        }
        let mut total = aggregate(
            self.latest.values(),
            &self.run_id,
            &self.test_type,
            collected_at,
        );
        let all_available = reported == self.expected;
        if !all_available && total.state.as_deref() == Some("Completed") {
            total.state = Some("Incomplete".to_string());
        }
        // 누락 전후의 서로 다른 Worker 집합을 비교하면 전체 증분과 처리량이 왜곡되므로 기준을 초기화한다.
        if !all_available || !self.previous_total_available {
            self.baselines.remove("total");
        }
        self.write_row(
            &mut text,
            "",
            "total",
            Some(&total),
            (!all_available).then_some("일부 Worker 통계 누락"),
            collected_at,
            reported,
            all_available,
        );
        self.previous_total_available = all_available;
        self.csv.write_all(text.as_bytes())?;
        self.csv.flush()?;
        Ok(())
    }

    /// 조회 결과의 가용 여부와 마지막 누적값만 반영한다.
    pub fn observe(&mut self, samples: &[WorkerSample]) {
        for sample in samples {
            self.availability
                .set(&sample.worker_id, sample.snapshot.is_some());
            if let Some(snapshot) = &sample.snapshot {
                self.latest.set(&sample.worker_id, snapshot.clone());
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_row(
        &mut self,
        out: &mut String,
        worker_id: &str,
        scope: &str,
        snapshot: Option<&RunSnapshot>,
        error: Option<&str>,
        at: DotnetDateTimeOffset,
        reported: usize,
        available: bool,
    ) {
        let key = if scope == "total" { "total" } else { worker_id };
        // 개별 처리량은 Worker의 표본 시각, 전체 처리량은 Controller의 수집 시각을 기준으로 한다.
        let sample_at = if scope == "total" {
            Some(at)
        } else {
            snapshot.map(|s| s.sample_at_utc)
        };
        let values = snapshot.map(|s| counts(&result_of(s)));
        let mut interval = None;
        // 첫 표본이나 계산할 수 없는 구간은 0 대신 빈 칸(콘솔 N/A)으로 남긴다.
        let mut rates = [None; 5];
        let mut increments = [None; 5];
        if let (true, Some(sample_at), Some(values)) = (available, sample_at, &values) {
            if let Some((before_time, before_counts)) = self.baselines.get(key)
                && sample_at > *before_time
            {
                let seconds = ticks_between(*before_time, sample_at) as f64 / 10_000_000.0;
                interval = Some(seconds);
                for i in 0..5 {
                    if values[i * 2] >= before_counts[i * 2] {
                        let increment = values[i * 2] - before_counts[i * 2];
                        increments[i] = Some(increment);
                        rates[i] = Some(increment as f64 / seconds);
                    }
                }
            }
            self.baselines.insert(key.to_string(), (sample_at, *values));
        }
        let file_size = snapshot.map(|s| result_of(s).file_size);
        let mut row = vec![
            self.run_id.clone(),
            self.sequence.to_string(),
            self.test_type.clone(),
            scope.to_string(),
            worker_id.to_string(),
            at.to_o_text(),
            sample_at.map(|t| t.to_o_text()).unwrap_or_default(),
            snapshot
                .map(|s| double_text(s.elapsed_seconds))
                .unwrap_or_default(),
            interval.map(double_text).unwrap_or_default(),
            snapshot
                .and_then(|s| s.state.clone())
                .unwrap_or_else(|| "Unavailable".to_string()),
            available.to_string(),
            snapshot.is_some_and(RunSnapshot::is_final).to_string(),
            error
                .map(str::to_string)
                .or_else(|| snapshot.and_then(|s| s.error.clone()))
                .unwrap_or_default(),
        ];
        for (i, rate) in rates.iter().enumerate() {
            row.push(values.map(|v| v[i * 2].to_string()).unwrap_or_default());
            row.push(values.map(|v| v[i * 2 + 1].to_string()).unwrap_or_default());
            row.push(rate.map(double_text).unwrap_or_default());
        }
        for rate in [rates[0], rates[1]] {
            row.push(
                rate.zip(file_size)
                    .map(|(rate, size)| double_text(rate * size as f64))
                    .unwrap_or_default(),
            );
        }
        row.push(self.expected.to_string());
        row.push(reported.to_string());
        let line: Vec<String> = row.into_iter().map(|v| csv_escape(&v)).collect();
        out.push_str(&line.join(","));
        out.push_str(NEW_LINE);
        if let (true, Some(snapshot)) = (scope == "total", snapshot)
            && !snapshot.is_final()
        {
            console::log(&console::format(
                snapshot,
                reported,
                self.expected,
                false,
                &FormatOptions {
                    rates: Some(rates),
                    state: None,
                    error: error.map(str::to_string),
                    increments: Some(increments),
                },
            ));
        }
    }

    /// 마지막으로 받은 통계를 저장한다. 정상 완료 여부는 전체 Worker의 최종 상태까지 확인한다.
    pub fn save_final(
        &mut self,
        state: &str,
        error: Option<&str>,
        settings: Option<&ResultSettings>,
    ) -> Result<(), ScenarioError> {
        let total = aggregate(
            self.latest.values(),
            &self.run_id,
            &self.test_type,
            DotnetDateTimeOffset::now(),
        );
        let starts: Vec<DotnetDateTimeOffset> = self
            .latest
            .values()
            .filter_map(|s| s.started_at_utc)
            .collect();
        let reported = self.availability.values().filter(|v| **v).count();
        let total_result = result_of(&total);
        let report = Report {
            run_id: &self.run_id,
            test_type: &self.test_type,
            state,
            error,
            settings,
            expected_workers: self.expected,
            reported_workers: reported,
            unavailable_workers: self
                .availability
                .0
                .iter()
                .filter(|(_, v)| !*v)
                .map(|(k, _)| k.as_str())
                .collect(),
            complete: reported == self.expected
                && self.latest.0.len() == self.expected
                && self
                    .latest
                    .values()
                    .all(|s| s.state.as_deref() == Some("Completed"))
                && state == "Completed",
            start_skew_milliseconds: starts
                .iter()
                .max()
                .zip(starts.iter().min())
                .map(|(max, min)| ticks_between(*min, *max) as f64 / 10_000.0),
            success_ops_per_second: (total.elapsed_seconds > 0.0).then(|| {
                (total_result.total - total_result.total_failed) as f64 / total.elapsed_seconds
            }),
            total: &total,
            workers: self.latest.values().collect(),
        };
        let text = to_dotnet_json(&report);
        self.json.seek(std::io::SeekFrom::Start(0))?;
        self.json.set_len(0)?;
        self.json.write_all(text.as_bytes())?;
        self.json.sync_all()?;
        if !self.final_logged && matches!(state, "Completed" | "Cancelled" | "Failed") {
            console::log(&console::format(
                &total,
                reported,
                self.expected,
                true,
                &FormatOptions {
                    state: Some(state.to_string()),
                    error: error.map(str::to_string),
                    ..FormatOptions::default()
                },
            ));
            self.final_logged = true;
        }
        Ok(())
    }
}

/// `RunSnapshot.Result`(없으면 기본값으로 본다).
fn result_of(snapshot: &RunSnapshot) -> RunResult {
    snapshot.result.clone().unwrap_or_default()
}

/// 두 시각 사이의 틱(100ns) 수.
fn ticks_between(from: DotnetDateTimeOffset, to: DotnetDateTimeOffset) -> i64 {
    (to.value() - from.value())
        .num_nanoseconds()
        .map_or(i64::MAX, |n| n / 100)
}

/// 누적 건수는 합산하고, 경과 시간은 최초 시작부터 마지막 완료(실행 중이면 현재)까지 계산한다.
pub fn aggregate<'a>(
    input: impl IntoIterator<Item = &'a RunSnapshot>,
    run_id: &str,
    test_type: &str,
    now: DotnetDateTimeOffset,
) -> RunSnapshot {
    let snapshots: Vec<&RunSnapshot> = input.into_iter().collect();
    let mut result = RunResult {
        test_type: Some(test_type.to_string()),
        start_time: DotnetDateTime::default(),
        end_time: DotnetDateTime::default(),
        bucket_type: None,
        bucket_name: None,
        object_prefix: None,
        thread_prefix: None,
        ..RunResult::default()
    };
    for s in &snapshots {
        let r = result_of(s);
        result.read += r.read;
        result.read_failed += r.read_failed;
        result.write += r.write;
        result.write_failed += r.write_failed;
        result.head += r.head;
        result.head_failed += r.head_failed;
        result.delete += r.delete;
        result.delete_failed += r.delete_failed;
        result.list += r.list;
        result.list_failed += r.list_failed;
        result.thread_count = result.thread_count.wrapping_add(r.thread_count);
        result.file_size = r.file_size;
    }
    result.total = counts(&result).iter().sum();
    result.total_failed = result.read_failed
        + result.write_failed
        + result.head_failed
        + result.delete_failed
        + result.list_failed;
    let starts: Vec<DotnetDateTimeOffset> =
        snapshots.iter().filter_map(|s| s.started_at_utc).collect();
    let min_start = starts.iter().min().copied();
    let done = !snapshots.is_empty() && snapshots.iter().all(|s| s.is_final());
    let end = if done {
        snapshots
            .iter()
            .map(|s| s.completed_at_utc.unwrap_or(now))
            .max()
            .unwrap_or(now)
    } else {
        now
    };
    let elapsed = min_start.map_or(0.0, |start| {
        (ticks_between(start, end) as f64 / 10_000_000.0).max(0.0)
    });
    if let Some(start) = min_start {
        result.start_time = DotnetDateTime::utc(start.value());
    }
    if done {
        result.end_time = DotnetDateTime::utc(end.value());
    }
    result.time = elapsed as i32;
    let has = |state: &str| snapshots.iter().any(|s| s.state.as_deref() == Some(state));
    let state = if has("Failed") {
        "Failed"
    } else if has("Cancelled") {
        "Cancelled"
    } else if done {
        "Completed"
    } else if min_start.is_some() {
        "Running"
    } else {
        "Preparing"
    };
    RunSnapshot {
        run_id: Some(run_id.to_string()),
        worker_id: None,
        test_type: Some(test_type.to_string()),
        state: Some(state.to_string()),
        error: None,
        sample_at_utc: now,
        scheduled_at_utc: None,
        started_at_utc: min_start,
        issuing_stopped_at_utc: if done {
            snapshots
                .iter()
                .filter_map(|s| s.issuing_stopped_at_utc)
                .max()
        } else {
            None
        },
        completed_at_utc: done.then_some(end),
        elapsed_seconds: elapsed,
        result: Some(result),
    }
}

/// Operations 순서마다 성공/실패를 교대로 배치한다. CSV 열과 증분 계산이 이 순서에 의존한다.
pub fn counts(r: &RunResult) -> [i64; 10] {
    [
        r.read,
        r.read_failed,
        r.write,
        r.write_failed,
        r.head,
        r.head_failed,
        r.delete,
        r.delete_failed,
        r.list,
        r.list_failed,
    ]
}

/// 원본 `Format`의 CSV 인용: `,` `"` 줄바꿈이 있으면 따옴표로 감싼다.
fn csv_escape(text: &str) -> String {
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// `File.Exists` 대신 새 파일만 만든다(`FileMode.CreateNew`, `FileShare.Read`).
fn create_new(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(1);
    }
    options.open(path)
}

/// 마지막 경로 요소의 `.` 위치(구분자를 만나면 없음).
fn extension_start(path: &Path) -> Option<usize> {
    let text = path.to_string_lossy();
    let bytes = text.as_bytes();
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'.' => return Some(i),
            b'/' | b'\\' | b':' => return None,
            _ => {}
        }
    }
    None
}

/// `Path.HasExtension`: 끝이 `.`이 아닌 확장자가 있는지.
fn has_extension(path: &Path) -> bool {
    extension_start(path).is_some_and(|i| i + 1 < path.to_string_lossy().len())
}

/// `Path.GetExtension`(`.json`, 없으면 빈 문자열).
fn get_extension(path: &Path) -> String {
    let text = path.to_string_lossy();
    match extension_start(path) {
        Some(i) if i + 1 < text.len() => text[i..].to_string(),
        _ => String::new(),
    }
}

/// `Path.ChangeExtension(path, ".csv")`.
fn change_extension_csv(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let stem = match extension_start(path) {
        Some(i) => &text[..i],
        None => &text,
    };
    PathBuf::from(format!("{stem}.csv"))
}

/// 파일 입출력 오류의 .NET 예외. FileMode.CreateNew가 막힌 경우(IOException: The file '...' already exists.)만 직접 만든다.
fn io_error(path: &Path, error: &std::io::Error) -> ScenarioError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        return ScenarioError::new(
            "System.IO.IOException",
            format!("The file '{}' already exists.", full_path(path).display()),
        );
    }
    path_error(path, error)
}
