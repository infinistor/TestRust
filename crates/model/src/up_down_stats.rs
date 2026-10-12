//! TESTCore `Data/UpDownStats.cs` 이식: 클라이언트 통계를 모아 진행 상황·최종 결과를 출력한다.
//!
//! 출력 문자열은 원본 `log.Info(...)` 메시지와 글자 단위로 같아야 한다. `*_message`가 메시지를 만들고,
//! 같은 이름의 `print_*`가 INFO 로그로 남긴다.
//!
//! 원본 그대로 둔 점: `PrintMix`는 Read 줄에 Write의 급변 표시를, Write 줄에 Read의 급변 표시를 쓴다.

use rust_decimal::Decimal;
use testrust_common::dotnet_format::{align, decimal_text, fixed_aligned};
use testrust_config::{EnumBucketTypes, MainConfig, UpDownConfig};
use tracing::info;

use crate::stats::{TestClient, TimeStats, WriteTimeStats};
use crate::units::file_size_unit;
use crate::up_down_result::UpDownResult;

const SEPARATOR: &str = "\n--------------------------------------------------------------";
pub const FORMAT_VALUE: i32 = 9;
pub const FORMAT_ERROR_VALUE: i32 = 5;
pub const FORMAT_AVERAGE_VALUE: i32 = 6;

/// `{value,9}` (정수)
fn v(value: i64) -> String {
    align(value.to_string(), FORMAT_VALUE)
}

/// `{value,9}` (decimal, 소수 자릿수 유지)
fn vd(value: Decimal) -> String {
    align(decimal_text(value), FORMAT_VALUE)
}

/// `{value,5}`
fn e(value: i64) -> String {
    align(value.to_string(), FORMAT_ERROR_VALUE)
}

/// `{value,9:F3}`
fn f3(value: Decimal) -> String {
    fixed_aligned(value, FORMAT_VALUE, 3)
}

/// `{value,6:F1}`
fn f1(value: Decimal) -> String {
    fixed_aligned(value, FORMAT_AVERAGE_VALUE, 1)
}

/// `Utility.GetFileSizeUint(value)`
fn size(value: Decimal) -> String {
    file_size_unit(value, false, false)
}

/// `Utility.GetFileSizeUint(value, abbreviation: true)`
fn size_short(value: Decimal) -> String {
    file_size_unit(value, true, false)
}

/// `times == 0 ? 0 : count / times`
fn per(count: i64, times: Decimal) -> Decimal {
    if times.is_zero() {
        Decimal::ZERO
    } else {
        Decimal::from(count) / times
    }
}

/// `(+ {GetLastSecondCount()}{Icon})`
fn last(stats: &TimeStats) -> String {
    format!(
        "(+ {}{})",
        decimal_text(stats.get_last_second_count()),
        stats.is_sharp_change().icon()
    )
}

/// .NET `Enum.ToString()`: 정의된 값은 이름, 아니면 숫자.
fn enum_to_string(value: EnumBucketTypes) -> String {
    match value.name() {
        "Unknown" => value.0.to_string(),
        name => name.to_string(),
    }
}

/// 원본 `UpDownStats`.
#[derive(Debug, Clone)]
pub struct UpDownStats {
    pub file_size: i64,
    pub write: WriteTimeStats,
    pub read: TimeStats,
    pub head: TimeStats,
    pub delete: TimeStats,
    pub list: TimeStats,
}

impl UpDownStats {
    pub fn new(file_size: i64) -> Self {
        Self {
            file_size,
            write: WriteTimeStats::default(),
            read: TimeStats::default(),
            head: TimeStats::default(),
            delete: TimeStats::default(),
            list: TimeStats::default(),
        }
    }

    fn aws_total_count(&self) -> i64 {
        self.head.total()
            + self.read.total()
            + self.write.total()
            + self.delete.total()
            + self.list.total()
    }

    fn aws_total_error(&self) -> i64 {
        self.head.error + self.read.error + self.write.error + self.delete.error + self.list.error
    }

    /// 원본 `Update`: 합계를 다시 내고 구간 평균을 갱신한다.
    pub fn update<C: TestClient + ?Sized>(&mut self, clients: &[&C]) {
        self.write.init();
        self.read.init();
        self.head.init();
        self.delete.init();
        self.list.init();
        for client in clients {
            let stats = client.stats();
            self.write.add(&stats.write);
            self.read.add(&stats.read);
            self.head.add(&stats.head);
            self.delete.add(&stats.delete);
            self.list.add(&stats.list);
        }
        self.write.update_average();
        self.write.update_part_average();
        self.read.update_average();
        self.head.update_average();
        self.delete.update_average();
        self.list.update_average();
    }

    /// 원본 `ToUpDownResult`.
    pub fn to_up_down_result(
        &self,
        test_type: &str,
        config: &UpDownConfig,
        main_config: &MainConfig,
        execution_time: Decimal,
    ) -> UpDownResult {
        let lower = test_type.to_lowercase();
        let is_list_object_test = lower.contains("listobject");
        let is_aws_test = lower.contains("aws");
        let read_success = if is_list_object_test {
            self.list.success
        } else {
            self.read.success
        };
        let read_failed = if is_list_object_test {
            self.list.error
        } else {
            self.read.error
        };
        let total = if is_aws_test {
            self.aws_total_count()
        } else {
            read_success + read_failed + self.write.total() + self.delete.total()
        };
        let total_failed = if is_aws_test {
            self.aws_total_error()
        } else {
            read_failed + self.write.error + self.delete.error
        };
        UpDownResult {
            test_type: test_type.to_string(),
            file_size: self.file_size,
            // (int)decimal은 0 쪽으로 버린다.
            time: execution_time.trunc().try_into().unwrap_or(0),
            read: read_success,
            read_failed,
            head: self.head.success,
            head_failed: self.head.error,
            write: self.write.success,
            write_failed: self.write.error,
            delete: self.delete.success,
            delete_failed: self.delete.error,
            list: self.list.success,
            list_failed: self.list.error,
            total,
            total_failed,
            thread_count: config.thread_count,
            read_ratio: config.read_ratio,
            write_ratio: config.write_ratio,
            delete_ratio: config.delete_ratio,
            bucket_type: enum_to_string(config.bucket_type),
            bucket_name: main_config.bucket_name.clone(),
            object_prefix: main_config.object_prefix.clone(),
            thread_prefix: main_config.thread_prefix.clone(),
            ..UpDownResult::default()
        }
    }

    pub fn prepare_message(&self, total: i64, times: Decimal) -> String {
        let w = &self.write;
        format!(
            "{SEPARATOR}\n Remaining Count : {}\n Write Count     : {} {}\n Write Average   : {} file/sec\n Bandwidth       : {}/s\n Times           : {} sec{SEPARATOR}",
            v(total - w.total()),
            v(w.total()),
            last(w),
            f3(w.get_average_per_second()),
            size(w.get_bandwidth(self.file_size)),
            f3(times)
        )
    }

    pub fn write_message(&self, times: Decimal) -> String {
        let w = &self.write;
        format!(
            "{SEPARATOR}\n Total Count   : {}\n Write Count   : {} {}\n Write Error   : {}\n Write Average : {} file/sec\n Bandwidth     : {}/s\n Success Ratio : {} %\n Times         : {} sec{SEPARATOR}",
            v(w.total()),
            v(w.success),
            last(w),
            v(w.error),
            f3(w.get_average_per_second()),
            size(w.get_bandwidth(self.file_size)),
            f3(w.success_ratio()),
            f3(times)
        )
    }

    pub fn read_message(&self, times: Decimal) -> String {
        let r = &self.read;
        format!(
            "{SEPARATOR}\n Total Count   : {}\n Read Count    : {} {}\n Read Error    : {}\n Read Average  : {} file/sec\n Bandwidth     : {}/s\n Success Ratio : {} %\n Times         : {} sec{SEPARATOR}",
            v(r.total()),
            v(r.success),
            last(r),
            v(r.error),
            f3(r.get_average_per_second()),
            size(r.get_bandwidth(self.file_size)),
            f3(r.success_ratio()),
            f3(times)
        )
    }

    pub fn read_total_message(&self, total: i64, times: Decimal) -> String {
        let r = &self.read;
        format!(
            "{SEPARATOR}\n Remaining Count : {}\n Read Count      : {} {}\n Read Error      : {}\n Read Average    : {} file/sec\n Bandwidth       : {}/s\n Success Ratio   : {} %\n Times           : {} sec{SEPARATOR}",
            v(total - r.total()),
            v(r.success),
            last(r),
            v(r.error),
            f3(r.get_average_per_second()),
            size(r.get_bandwidth(self.file_size)),
            f3(r.success_ratio()),
            f3(times)
        )
    }

    pub fn list_object_message(&self, times: Decimal) -> String {
        let l = &self.list;
        format!(
            "{SEPARATOR}\n Total Count    : {}\n List Count     : {} {}\n List Error     : {}\n List Average   : {} list/sec\n Bandwidth      : {}/s\n Success Ratio  : {} %\n Times          : {} sec{SEPARATOR}",
            v(l.total()),
            v(l.success),
            last(l),
            v(l.error),
            f3(l.get_average_per_second()),
            size(l.get_bandwidth(self.file_size)),
            f3(l.success_ratio()),
            f3(times)
        )
    }

    pub fn head_message(&self, times: Decimal) -> String {
        let h = &self.head;
        format!(
            "{SEPARATOR}\n Total Count   : {}\n Head Count    : {} {}\n Head Error    : {}\n Head Average  : {} file/sec\n Bandwidth     : {}/s\n Success Ratio : {} %\n Times         : {} sec{SEPARATOR}",
            v(h.total()),
            v(h.success),
            last(h),
            v(h.error),
            f3(h.get_average_per_second()),
            size(h.get_bandwidth(self.file_size)),
            f3(h.success_ratio()),
            f3(times)
        )
    }

    pub fn delete_message(&self, times: Decimal) -> String {
        let d = &self.delete;
        format!(
            "{SEPARATOR}\n Total Count    : {}\n Delete Count   : {} {}\n Delete Error   : {}\n Delete Average : {} file/sec\n Bandwidth      : {}/s\n Success Ratio  : {} %\n Times          : {} sec{SEPARATOR}",
            v(d.total()),
            v(d.success),
            last(d),
            v(d.error),
            f3(d.get_average_per_second()),
            size(d.get_bandwidth(self.file_size)),
            f3(d.success_ratio()),
            f3(times)
        )
    }

    pub fn delete_total_message(&self, total: i64, times: Decimal) -> String {
        let d = &self.delete;
        format!(
            "{SEPARATOR}\n Remaining Count  : {}\n Delete Count     : {} {}\n Delete Error     : {}\n Delete Average   : {} file/sec\n Bandwidth        : {}/s\n Success Ratio    : {} %\n Times            : {} sec{SEPARATOR}",
            v(total - d.total()),
            v(d.success),
            last(d),
            v(d.error),
            f3(d.get_average_per_second()),
            size(d.get_bandwidth(self.file_size)),
            f3(d.success_ratio()),
            f3(times)
        )
    }

    fn part_last(&self) -> String {
        format!(
            "(+ {}{})",
            decimal_text(self.write.get_part_last_second_count()),
            self.write.is_part_sharp_change().icon()
        )
    }

    pub fn multi_upload_message(&self, total: i64, times: Decimal, part_size: i64) -> String {
        let w = &self.write;
        format!(
            "{SEPARATOR}\n Remaining Count  : {}\n Upload Count     : {} {}\n Upload Average   : {} file/sec\n Upload Bandwidth : {}/s\n Part Count       : {} {}\n Part Average     : {} part/sec\n Part Bandwidth   : {}/s\n Upload Error     : {}\n Success Ratio    : {} %\n Times            : {} sec{SEPARATOR}",
            v(total - w.total()),
            v(w.success),
            last(w),
            f3(w.get_average_per_second()),
            size(w.get_bandwidth(self.file_size)),
            vd(w.part),
            self.part_last(),
            f3(w.get_part_average_per_second()),
            size(w.get_part_bandwidth(part_size)),
            v(w.error),
            f3(w.success_ratio()),
            f3(times)
        )
    }

    pub fn multi_upload_v2_message(&self, total: i64, times: Decimal, part_size: i64) -> String {
        let w = &self.write;
        let r = &self.read;
        format!(
            "{SEPARATOR}\n Remaining Count : {}\n Upload Count    : {} {}\n Upload Average  : {} file/sec\n Upload Bandwidth: {}/s\n Part Count      : {} {}\n Part Average    : {} part/sec\n Part Bandwidth  : {}/s\n Upload Error    : {}\n Success Ratio   : {} %\n Read Count      : {} {}\n Read Error      : {}\n Read Average    : {} file/sec\n Bandwidth       : {}/s\n Success Ratio   : {} %\n Times           : {} sec{SEPARATOR}",
            v(total - w.total()),
            v(w.success),
            last(w),
            f3(w.get_average_per_second()),
            size(w.get_bandwidth(self.file_size)),
            vd(w.part),
            self.part_last(),
            f3(w.get_part_average_per_second()),
            size(w.get_part_bandwidth(part_size)),
            v(w.error),
            f3(w.success_ratio()),
            v(r.success),
            last(r),
            v(r.error),
            f3(r.get_average_per_second()),
            size(r.get_bandwidth(self.file_size)),
            f3(r.success_ratio()),
            f3(times)
        )
    }

    pub fn download_message(&self, total: i64, times: Decimal) -> String {
        let r = &self.read;
        format!(
            "{SEPARATOR}\n Remaining Count  : {}\n Download Count   : {} {}\n Download Error   : {}\n Download Average : {} file/sec\n Bandwidth        : {}/s\n Success Ratio    : {} %\n Times            : {} sec{SEPARATOR}",
            v(total - r.total()),
            v(r.success),
            last(r),
            v(r.error),
            f3(r.get_average_per_second()),
            size(r.get_bandwidth(self.file_size)),
            f3(r.success_ratio()),
            f3(times)
        )
    }

    /// `Read Count  : ... (+ {last,9}{icon}) Error : ... Average : ... file/sec Bandwidth : .../s` 한 줄.
    fn summary_line(
        &self,
        label: &str,
        stats: &TimeStats,
        icon_from: &TimeStats,
        unit: &str,
        bandwidth: bool,
    ) -> String {
        let mut line = format!(
            "\n {label}: {} (+ {}{}) Error : {} Average : {} {unit}",
            v(stats.success),
            align(decimal_text(stats.get_last_second_count()), FORMAT_VALUE),
            icon_from.is_sharp_change().icon(),
            e(stats.error),
            f1(stats.get_average_per_second()),
        );
        if bandwidth {
            line.push_str(&format!(
                " Bandwidth : {}/s",
                size_short(stats.get_bandwidth(self.file_size))
            ));
        }
        line
    }

    pub fn mix_message(&self, times: Decimal) -> String {
        let total_count = self.read.total() + self.write.total();
        let total_error = self.read.error + self.write.error;
        let total_average = per(total_count, times);
        let total_bandwidth = total_average * Decimal::from(self.file_size);
        format!(
            "{SEPARATOR}{}{}\n Total Count : {} Error : {} Average : {} file/sec Bandwidth : {}/s\n Total Execution Time : {} sec{SEPARATOR}",
            // 원본 그대로: Read 줄은 Write의 급변 표시, Write 줄은 Read의 급변 표시를 쓴다.
            self.summary_line("Read Count  ", &self.read, &self.write, "file/sec", true),
            self.summary_line("Write Count ", &self.write, &self.read, "file/sec", true),
            v(total_count),
            e(total_error),
            f1(total_average),
            size_short(total_bandwidth),
            f3(times)
        )
    }

    pub fn all_message(&self, times: Decimal) -> String {
        let total_count = self.read.total() + self.write.total();
        let total_error = self.read.error + self.write.error;
        let total_average = per(total_count, times);
        format!(
            "{SEPARATOR}{}{}{}\n Total Count  : {} Error : {} Average : {} file/sec\n Total Execution Time : {} sec{SEPARATOR}",
            self.summary_line("Read Count  ", &self.read, &self.read, "file/sec", true),
            self.summary_line("Write Count ", &self.write, &self.write, "file/sec", true),
            self.summary_line(
                "Delete Count ",
                &self.delete,
                &self.delete,
                "file/sec",
                true
            ),
            v(total_count),
            e(total_error),
            f1(total_average),
            f3(times)
        )
    }

    pub fn aws_message(&self, times: Decimal) -> String {
        let total_count = self.aws_total_count();
        let total_error = self.aws_total_error();
        let total_average = per(total_count, times);
        format!(
            "{SEPARATOR}{}{}{}{}{}\n Total Count : {} Error : {} Average : {} op/sec\n Total Execution Time : {} sec{SEPARATOR}",
            self.summary_line("Head Count  ", &self.head, &self.head, "file/sec", true),
            self.summary_line("Read Count  ", &self.read, &self.read, "file/sec", true),
            self.summary_line("Write Count ", &self.write, &self.write, "file/sec", true),
            self.summary_line("Delete Count", &self.delete, &self.delete, "file/sec", true),
            self.summary_line("List Count  ", &self.list, &self.list, "list/sec", false),
            v(total_count),
            e(total_error),
            f1(total_average),
            f3(times)
        )
    }

    pub fn prepare_final_message(&self, total: i64, times: Decimal) -> String {
        let w = &self.write;
        let average = per(w.success, times);
        format!(
            "{SEPARATOR}\n [PREPARE TEST FINAL RESULTS]\n Total Target    : {}\n Write Success   : {}\n Write Error     : {}\n Total Average   : {} file/sec\n Total Bandwidth : {}/s\n Success Ratio   : {} %\n Total Time      : {} sec{SEPARATOR}",
            v(total),
            v(w.success),
            v(w.error),
            f3(average),
            size(average * Decimal::from(self.file_size)),
            f3(w.success_ratio()),
            f3(times)
        )
    }

    /// 쓰기·읽기·Head·삭제·목록 최종 결과 공통 형식.
    fn final_message(
        &self,
        title: &str,
        label: &str,
        stats: &TimeStats,
        unit: &str,
        times: Decimal,
    ) -> String {
        let average = per(stats.success, times);
        format!(
            "{SEPARATOR}\n [{title} TEST FINAL RESULTS]\n {label} Success{} : {}\n {label} Error{} : {}\n Total Average   : {} {unit}\n Total Bandwidth : {}/s\n Success Ratio   : {} %\n Total Time      : {} sec{SEPARATOR}",
            " ".repeat(7 - label.len()),
            v(stats.success),
            " ".repeat(9 - label.len()),
            v(stats.error),
            f3(average),
            size(average * Decimal::from(self.file_size)),
            f3(stats.success_ratio()),
            f3(times)
        )
    }

    pub fn write_final_message(&self, times: Decimal) -> String {
        self.final_message("WRITE", "Write", &self.write, "file/sec", times)
    }

    pub fn read_final_message(&self, times: Decimal) -> String {
        self.final_message("READ", "Read", &self.read, "file/sec", times)
    }

    pub fn list_object_final_message(&self, times: Decimal) -> String {
        self.final_message("LIST OBJECT", "List", &self.list, "list/sec", times)
    }

    pub fn head_final_message(&self, times: Decimal) -> String {
        self.final_message("HEAD", "Head", &self.head, "file/sec", times)
    }

    pub fn delete_final_message(&self, times: Decimal) -> String {
        self.final_message("DELETE", "Delete", &self.delete, "file/sec", times)
    }

    /// 전체 대상 수와 남은 수가 들어가는 최종 결과(`ReadV2`, `DeleteV2`).
    fn final_total_message(
        &self,
        title: &str,
        label: &str,
        stats: &TimeStats,
        total: i64,
        times: Decimal,
    ) -> String {
        let average = per(stats.success, times);
        format!(
            "{SEPARATOR}\n [{title} TEST FINAL RESULTS]\n Total Target    : {}\n {label} Success{} : {}\n {label} Error{} : {}\n Remaining       : {}\n Total Average   : {} file/sec\n Total Bandwidth : {}/s\n Success Ratio   : {} %\n Total Time      : {} sec{SEPARATOR}",
            v(total),
            " ".repeat(7 - label.len()),
            v(stats.success),
            " ".repeat(9 - label.len()),
            v(stats.error),
            v(total - stats.total()),
            f3(average),
            size(average * Decimal::from(self.file_size)),
            f3(stats.success_ratio()),
            f3(times)
        )
    }

    pub fn read_v2_final_message(&self, total: i64, times: Decimal) -> String {
        self.final_total_message("READ V2", "Read", &self.read, total, times)
    }

    pub fn delete_v2_final_message(&self, total: i64, times: Decimal) -> String {
        self.final_total_message("DELETE V2", "Delete", &self.delete, total, times)
    }

    /// `Read Success     : ... Error : ... Total Average : ... file/sec Total Bandwidth : .../s` 한 줄.
    fn final_summary_line(&self, label: &str, stats: &TimeStats, times: Decimal) -> String {
        let average = per(stats.success, times);
        format!(
            "\n {label}: {} Error : {} Total Average : {} file/sec Total Bandwidth : {}/s",
            v(stats.success),
            e(stats.error),
            f1(average),
            size_short(average * Decimal::from(self.file_size))
        )
    }

    pub fn mix_final_message(&self, times: Decimal) -> String {
        let total_count = self.read.total() + self.write.total();
        let total_error = self.read.error + self.write.error;
        let total_average = per(total_count, times);
        format!(
            "{SEPARATOR}\n [MIX TEST FINAL RESULTS]{}{}\n Total Count      : {} Error : {} Average : {} file/sec Bandwidth : {}/s\n Total Time       : {} sec{SEPARATOR}",
            self.final_summary_line("Read Success     ", &self.read, times),
            self.final_summary_line("Write Success    ", &self.write, times),
            v(total_count),
            e(total_error),
            f1(total_average),
            size_short(total_average * Decimal::from(self.file_size)),
            f3(times)
        )
    }

    pub fn all_final_message(&self, times: Decimal) -> String {
        let total_count = self.read.total() + self.write.total() + self.delete.total();
        let total_error = self.read.error + self.write.error + self.delete.error;
        let total_average = per(total_count, times);
        format!(
            "{SEPARATOR}\n [ALL TEST FINAL RESULTS]{}{}{}\n Total Count      : {} Error : {} Average : {} file/sec\n Total Time       : {} sec{SEPARATOR}",
            self.final_summary_line("Read Success     ", &self.read, times),
            self.final_summary_line("Write Success    ", &self.write, times),
            self.final_summary_line("Delete Success   ", &self.delete, times),
            v(total_count),
            e(total_error),
            f1(total_average),
            f3(times)
        )
    }
}

/// 메시지를 원본 `log.Info`처럼 INFO 로그로 남긴다.
pub fn log_info(message: &str) {
    info!("{message}");
}
