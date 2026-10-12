//! 체크섬 계산이 TESTCore `ChecksumCalculator`와 같은 값을 내는지 확인한다.
//! 기준 출력은 `tools/dotnet-oracle`의 `checksum` 명령으로 만든다.

use std::collections::BTreeMap;
use std::path::Path;

use testrust_s3::ChecksumAlgorithm;
use testrust_s3::checksum::{calculate_checksum, checksum_bytes};

const FIXTURES: &[&str] = &[
    "empty.bin",
    "zero.bin",
    "check.txt",
    "pattern-4095.bin",
    "pattern-4096.bin",
    "pattern-4097.bin",
    "pattern-100000.bin",
];

#[test]
fn checksum_matches_dotnet() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/parity");
    for name in FIXTURES {
        let path = root.join("checksum").join(name);
        let baseline =
            std::fs::read_to_string(root.join(format!("baseline/checksum/{name}.json"))).unwrap();
        let expected: BTreeMap<String, String> = serde_json::from_str(&baseline).unwrap();
        let data = std::fs::read(&path).unwrap();

        let mut actual = BTreeMap::new();
        for algorithm in ChecksumAlgorithm::ALL {
            if algorithm == ChecksumAlgorithm::None {
                continue;
            }
            let value = calculate_checksum(&path, algorithm).unwrap();
            assert_eq!(
                value,
                checksum_bytes(&data, algorithm),
                "{name} {algorithm}"
            );
            actual.insert(algorithm.name().to_string(), value);
        }
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn directory_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    assert!(calculate_checksum(dir.path(), ChecksumAlgorithm::Sha256).is_err());
}
