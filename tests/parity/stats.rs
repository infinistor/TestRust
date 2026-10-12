//! `UpDownStats`의 진행 상황·최종 결과 출력과 `UpDownResult` JSON이 TESTCore와 글자 단위로 같은지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `stats` 명령으로 만든다.

use std::path::Path;
use std::str::FromStr;

use chrono::{TimeZone, Utc};
use rust_decimal::Decimal;
use serde_json::Value;
use testrust_common::DotnetDateTime;
use testrust_common::dotnet_json::NEW_LINE;
use testrust_config::Config;
use testrust_model::{OperationStats, QuitFlag, TestClient, TestStats, UpDownStats};

#[derive(Default)]
struct FakeClient {
    stats: TestStats,
    quit: QuitFlag,
}

impl TestClient for FakeClient {
    fn stats(&self) -> &TestStats {
        &self.stats
    }

    fn quit(&self) -> bool {
        self.quit.get()
    }

    fn set_quit(&self, quit: bool) {
        self.quit.set(quit);
    }
}

fn apply(stats: &OperationStats, values: &Value) {
    stats.set_success(values[0].as_i64().unwrap());
    stats.set_error(values[1].as_i64().unwrap());
}

/// JSON 숫자를 .NET `decimal`과 같은 자릿수로 읽는다.
fn decimal(value: &Value) -> Decimal {
    Decimal::from_str(&value.to_string()).unwrap()
}

fn message(stats: &UpDownStats, print: &str, total: i64, part_size: i64, t: Decimal) -> String {
    match print {
        "Prepare" => stats.prepare_message(total, t),
        "Write" => stats.write_message(t),
        "Read" => stats.read_message(t),
        "ReadTotal" => stats.read_total_message(total, t),
        "ListObject" => stats.list_object_message(t),
        "Head" => stats.head_message(t),
        "Delete" => stats.delete_message(t),
        "DeleteTotal" => stats.delete_total_message(total, t),
        "MultiUpload" => stats.multi_upload_message(total, t, part_size),
        "MultiUploadV2" => stats.multi_upload_v2_message(total, t, part_size),
        "Download" => stats.download_message(total, t),
        "Mix" => stats.mix_message(t),
        "All" => stats.all_message(t),
        "AWS" => stats.aws_message(t),
        "PrepareFinal" => stats.prepare_final_message(total, t),
        "WriteFinal" => stats.write_final_message(t),
        "ReadFinal" => stats.read_final_message(t),
        "ListObjectFinal" => stats.list_object_final_message(t),
        "ReadV2Final" => stats.read_v2_final_message(total, t),
        "HeadFinal" => stats.head_final_message(t),
        "DeleteFinal" => stats.delete_final_message(t),
        "DeleteV2Final" => stats.delete_v2_final_message(total, t),
        "MixFinal" => stats.mix_final_message(t),
        "AllFinal" => stats.all_final_message(t),
        other => panic!("알 수 없는 출력: {other}"),
    }
}

#[test]
fn stats_output_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["mixed", "edge"] {
        let read = |path: String| -> Value {
            serde_json::from_str(&std::fs::read_to_string(root.join(path)).unwrap()).unwrap()
        };
        let spec = read(format!("tests/parity/stats/{name}.json"));
        let baseline = read(format!("tests/parity/baseline/stats/{name}.json"));
        let total = spec["total"].as_i64().unwrap();
        let part_size = spec["partSize"].as_i64().unwrap();
        let mut stats = UpDownStats::new(spec["fileSize"].as_i64().unwrap());

        for (index, step) in spec["steps"].as_array().unwrap().iter().enumerate() {
            let clients: Vec<FakeClient> = step["clients"]
                .as_array()
                .unwrap()
                .iter()
                .map(|values| {
                    let client = FakeClient::default();
                    if let Some(w) = values.get("write") {
                        apply(&client.stats.write, w);
                        if let Some(part) = w.get(2) {
                            client.stats.write.set_part(part.as_i64().unwrap());
                        }
                    }
                    for (key, target) in [
                        ("read", &client.stats.read),
                        ("head", &client.stats.head),
                        ("delete", &client.stats.delete),
                        ("list", &client.stats.list),
                    ] {
                        if let Some(values) = values.get(key) {
                            apply(target, values);
                        }
                    }
                    client
                })
                .collect();
            let refs: Vec<&FakeClient> = clients.iter().collect();
            stats.update(&refs);

            let times = decimal(&step["times"]);
            let expected = baseline["steps"][index]["messages"].as_array().unwrap();
            let prints = step["prints"].as_array().unwrap();
            assert_eq!(prints.len(), expected.len(), "{name} 단계 {index}");
            for (print, expected) in prints.iter().zip(expected) {
                let print = print.as_str().unwrap();
                let actual = message(&stats, print, total, part_size, times);
                assert_eq!(
                    actual,
                    expected.as_str().unwrap(),
                    "{name} 단계 {index} {print}"
                );
            }
        }

        let result_spec = &spec["result"];
        let config = Config::load(root.join(spec["config"].as_str().unwrap()), None).unwrap();
        let mut result = stats.to_up_down_result(
            result_spec["testType"].as_str().unwrap(),
            &config.up_down,
            &config.main,
            decimal(&result_spec["executionTime"]),
        );
        result.start_time = DotnetDateTime::utc(Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap());
        result.end_time = DotnetDateTime::utc(Utc.with_ymd_and_hms(2024, 1, 2, 3, 5, 6).unwrap());
        let expected = baseline["resultJson"]
            .as_str()
            .unwrap()
            .replace("\r\n", NEW_LINE);
        assert_eq!(result.to_json().unwrap(), expected, "{name} 결과 JSON");
    }
}

/// `DateTime.MinValue`(기본 `EndTime`)는 양의 오프셋 시간대에서 유닉스 초로 바꿀 수 없다(.NET 예외).
#[test]
fn min_value_end_time_fails_in_positive_offset() {
    let result = testrust_model::UpDownResult::default();
    let local_offset = chrono::Local::now().offset().local_minus_utc();
    assert_eq!(result.to_json().is_err(), local_offset > 0);
}
