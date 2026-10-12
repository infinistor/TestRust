//! TESTCore `Util/Utility.cs`의 크기 표시 함수(`GetFileSizeUint`, `GetFileSizeUintSimple`).

use rust_decimal::Decimal;
use testrust_common::dotnet_format::{fixed, fixed_aligned};

const SI_UNITS: [&str; 9] = ["Byte", "KB", "MB", "GB", "TB", "PB", "EB", "ZB", "YB"];
const IEC_UNITS: [&str; 9] = [
    "Byte", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB",
];
const MAX_VALUE: i64 = 1000;
const FORMAT_VALUE: i32 = 9;
const FORMAT_AVERAGE_VALUE: i32 = 6;

/// 원본 `GetFileSizeUint(decimal fileSize, bool abbreviation = false, bool si = false)`.
/// 값이 1000보다 큰 동안 1024(si면 1000)로 나눈다. 그래서 1001바이트는 `0.978 KiB`처럼 나온다.
pub fn file_size_unit(file_size: Decimal, abbreviation: bool, si: bool) -> String {
    let prefix = Decimal::from(if si { 1000 } else { 1024 });
    let mut size = file_size;
    let mut unit_count = 0;
    while size > Decimal::from(MAX_VALUE) {
        size /= prefix;
        unit_count += 1;
    }
    let unit = if si {
        SI_UNITS[unit_count]
    } else {
        IEC_UNITS[unit_count]
    };
    if abbreviation {
        format!("{} {unit}", fixed_aligned(size, FORMAT_AVERAGE_VALUE, 1))
    } else {
        format!("{} {unit}", fixed_aligned(size, FORMAT_VALUE, 3))
    }
}

/// 원본 `GetFileSizeUintSimple(decimal fileSize)`.
pub fn file_size_unit_simple(file_size: Decimal) -> String {
    let mut size = file_size;
    let mut unit_count = 0;
    while size > Decimal::from(MAX_VALUE) {
        size /= Decimal::from(1000);
        unit_count += 1;
    }
    format!("{} {}", fixed(size, 1), SI_UNITS[unit_count])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units() {
        assert_eq!(
            file_size_unit(Decimal::from(1000), false, false),
            " 1000.000 Byte"
        );
        assert_eq!(
            file_size_unit(Decimal::from(1001), false, false),
            "    0.978 KiB"
        );
        assert_eq!(
            file_size_unit(Decimal::from(10_485_760), true, false),
            "  10.0 MiB"
        );
        assert_eq!(file_size_unit_simple(Decimal::from(1_500_000)), "1.5 MB");
    }
}
