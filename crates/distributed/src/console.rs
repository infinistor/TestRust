//! 원본 `Distributed/ResultConsoleFormatter.cs`: 진행·최종 결과 콘솔 블록.
//!
//! 분산 합계를 기존 부하 테스트의 진행/최종 결과 형식(`{label,-15} : {value,9}`)으로 만든다. 줄바꿈은 원본처럼 `\n`이다.

use rust_decimal::Decimal;
use testrust_common::dotnet_format::{align, fixed};

use crate::contracts::RunSnapshot;

/// 원본 `Format`의 선택 인자. `rates`·`increments`는 Read, Write, Head, Delete, List 순서이고 `None` 항목은 계산할 수
/// 없다는 뜻이다.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FormatOptions {
    pub rates: Option<[Option<f64>; 5]>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub increments: Option<[Option<i64>; 5]>,
}

/// 메시지를 원본 `log.Info`처럼 INFO 로그로 남긴다.
pub fn log(message: &str) {
    testrust_model::up_down_stats::log_info(message);
}

/// `double.ToString("F3", InvariantCulture)`.
fn number(value: f64) -> String {
    match Decimal::from_f64_retain(value) {
        Some(decimal) => fixed(decimal, 3),
        None => format!("{value:.3}"),
    }
}

/// 진행 통계는 표본 간 증분 처리량, 최종 통계는 전체 경과 시간당 성공 건수를 사용한다.
pub fn format(
    snapshot: &RunSnapshot,
    reported: usize,
    expected: usize,
    final_: bool,
    options: &FormatOptions,
) -> String {
    let test_type = snapshot.test_type.as_deref().unwrap_or_default();
    let title = match test_type {
        "Put" => "WRITE".to_string(),
        "Get" => "READ".to_string(),
        other => other.to_uppercase(),
    };
    let result = snapshot.result.clone().unwrap_or_default();
    let mut text = format!(
        "\n[{title} TEST {}]\n",
        if final_ { "FINAL RESULTS" } else { "PROGRESS" }
    );
    let mut line = |label: &str, value: &str, suffix: &str| {
        text.push_str(&format!(
            "{} : {}{suffix}\n",
            align(label, -15),
            align(value, 9)
        ));
    };
    line("Worker Count", &format!("{reported}/{expected}"), "");
    line(
        "State",
        options
            .state
            .as_deref()
            .or(snapshot.state.as_deref())
            .unwrap_or_default(),
        "",
    );
    if reported < expected {
        line("Statistics", "Partial (last reported counts)", "");
    }

    let (mut success, mut failed) = (0i64, 0i64);
    // `double? rate = 0`: 한 번이라도 계산할 수 없는 값이 더해지면 끝까지 `None`이다.
    let mut rate = Some(0.0f64);
    let mut operation = |name: &str, count: i64, failures: i64, index: usize| {
        let increment = options.increments.and_then(|i| i[index]);
        let suffix = if final_ {
            String::new()
        } else {
            match increment {
                Some(value) => format!(" (+ {value})"),
                None => " (+ N/A)".to_string(),
            }
        };
        line(
            &format!("{name}{}", if final_ { " Success" } else { " Count" }),
            &count.to_string(),
            &suffix,
        );
        line(&format!("{name} Error"), &failures.to_string(), "");
        success += count;
        failed += failures;
        let current = if final_ {
            Some(if snapshot.elapsed_seconds > 0.0 {
                count as f64 / snapshot.elapsed_seconds
            } else {
                0.0
            })
        } else {
            options.rates.and_then(|r| r[index])
        };
        rate = rate.zip(current).map(|(a, b)| a + b);
        if !final_ || test_type == "Mix" {
            line(
                &format!("{name} Average"),
                &current.map_or_else(|| "N/A".to_string(), |c| format!("{} file/sec", number(c))),
                "",
            );
        }
    };
    match test_type {
        "Get" => operation("Read", result.read, result.read_failed, 0),
        "Delete" => operation("Delete", result.delete, result.delete_failed, 3),
        "Mix" => {
            operation("Read", result.read, result.read_failed, 0);
            operation("Write", result.write, result.write_failed, 1);
        }
        _ => operation("Write", result.write, result.write_failed, 1),
    }
    if final_ {
        line(
            "Total Average",
            &format!("{} file/sec", number(rate.unwrap_or(0.0))),
            "",
        );
    } else if test_type == "Mix" {
        line(
            "Total Average",
            &rate.map_or_else(|| "N/A".to_string(), |r| format!("{} file/sec", number(r))),
            "",
        );
    }
    // 대역폭은 설정 파일 크기와 성공 처리량의 곱으로 추정하며, 실제 네트워크 전송량은 측정하지 않는다.
    line(
        if final_ {
            "Total Bandwidth"
        } else {
            "Bandwidth"
        },
        &rate.map_or_else(
            || "N/A".to_string(),
            |r| {
                format!(
                    "{} MiB/s",
                    number(r * result.file_size as f64 / (1024 * 1024) as f64)
                )
            },
        ),
        "",
    );
    let ratio = if success + failed > 0 {
        success as f64 * 100.0 / (success + failed) as f64
    } else {
        0.0
    };
    line("Success Ratio", &format!("{} %", number(ratio)), "");
    line(
        if final_ { "Total Time" } else { "Times" },
        &format!("{} sec", number(snapshot.elapsed_seconds)),
        "",
    );
    if let Some(error) = options.error.as_deref().filter(|e| !e.trim().is_empty()) {
        line("Error", error, "");
    }
    text.push_str("--------------------------------------------------------------");
    text
}
