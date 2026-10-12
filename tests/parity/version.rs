//! `--version` 출력이 TESTCore와 같은 형식인지 확인한다.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_TestRust");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("TestRust 실행")
}

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 시각과 버전 문자열처럼 실행마다 달라지는 값을 자리표시자로 바꾼다.
fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .lines()
        .map(|line| {
            if let Some(rest) = line.strip_prefix("INFO  ") {
                let (time, message) = rest.split_at(19.min(rest.len()));
                assert!(is_timestamp(time), "시각 형식이 다름: {line}");
                format!("INFO  <TIME>{message}")
            } else if is_version(line) {
                "<VERSION>".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `yyyy-MM-dd HH:mm:ss`
fn is_timestamp(text: &str) -> bool {
    let pattern = "dddd-dd-dd dd:dd:dd";
    text.len() == pattern.len()
        && text.chars().zip(pattern.chars()).all(|(c, p)| match p {
            'd' => c.is_ascii_digit(),
            _ => c == p,
        })
}

/// `태그_커밋수_해시`
fn is_version(text: &str) -> bool {
    let mut parts = text.rsplitn(3, '_');
    let (Some(hash), Some(count), Some(tag)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !tag.is_empty()
        && !count.is_empty()
        && count.chars().all(|c| c.is_ascii_digit())
        && (hash == "unknown" || (!hash.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())))
}

#[test]
fn version_matches_dotnet_format() {
    let baseline = normalize(include_str!("baseline/version.txt"));
    for flag in ["--version", "-v"] {
        let output = run(&[flag]);
        assert_eq!(output.status.code(), Some(0), "{flag}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(normalize(&stdout), baseline, "{flag}");
    }
}

#[test]
fn version_uses_git_describe() {
    let (Some(count), Some(hash)) = (
        git(&["rev-list", "--count", "HEAD"]),
        git(&["rev-parse", "--short", "HEAD"]),
    ) else {
        return; // git 저장소 밖에서 빌드한 경우
    };
    let tag = git(&["describe", "--tags", "--abbrev=0"]).unwrap_or_else(|| "v0.0.0".into());
    let stdout = String::from_utf8(run(&["--version"]).stdout).unwrap();
    let version = stdout.lines().last().unwrap();
    assert_eq!(version, format!("{tag}_{count}_{hash}"));
}
