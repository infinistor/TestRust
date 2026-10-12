# 성능 비교용 Linux 묶음을 만든다(Docker Desktop 필요).
#
# - TestRust: 저장소 루트의 build-linux.ps1(x86_64-unknown-linux-musl 정적 바이너리, 대상 장비의 glibc와 무관).
# - TESTCore: TESTCore 저장소의 지정 커밋을 git archive로 내보내(원본 디렉터리는 건드리지 않는다)
#   `dotnet publish -r linux-x64 --self-contained`(대상 장비에 .NET 런타임이 없어도 된다).
# - dist/perf-bundle.tar.gz: TestRust, testcore/, compare.sh, README.txt(실행 권한은 컨테이너 안에서 묶을 때 준다).
#
# 사용법(저장소 루트에서): pwsh tools/perf/build-linux.ps1 [-Ref HEAD]
param(
	[string]$TestCore = "E:\Code\Git\TESTCore",
	[string]$Ref = "HEAD"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$dist = Join-Path $root "dist"
$bundle = Join-Path $dist "perf-bundle"
if (Test-Path $bundle) { Remove-Item -Recurse -Force $bundle }
New-Item -ItemType Directory -Force (Join-Path $bundle "testcore") | Out-Null

# TestRust(정적 musl): 저장소 루트의 build-linux.ps1로 만든다.
& (Join-Path $root "build-linux.ps1") -SkipDeploy
if ($LASTEXITCODE -ne 0) { throw "TestRust Linux 빌드 실패" }
Copy-Item (Join-Path $root "dist\linux\TestRust\TestRust") $bundle

# TESTCore(self-contained linux-x64)
$commit = git -C $TestCore rev-parse --short "$Ref^{commit}"
$src = Join-Path ([IO.Path]::GetTempPath()) "testrust-testcore-linux-$commit"
if (Test-Path $src) { Remove-Item -Recurse -Force $src }
New-Item -ItemType Directory -Force $src | Out-Null
$tar = Join-Path ([IO.Path]::GetTempPath()) "testcore-$commit.tar"
git -C $TestCore archive --format=tar $Ref -o $tar
tar -xf $tar -C $src
Remove-Item $tar
dotnet publish (Join-Path $src "TESTCore.csproj") -c Release -r linux-x64 --self-contained true -o (Join-Path $bundle "testcore") -v q -nologo | Out-Host
if ($LASTEXITCODE -ne 0) { throw "TESTCore Linux 게시 실패" }
Remove-Item -Recurse -Force $src

Copy-Item (Join-Path $PSScriptRoot "compare.sh") $bundle
@"
TESTCore(.NET, $commit) vs TestRust(Rust) 성능 비교 묶음

1. 이 디렉터리에 두 도구의 KSAN 설정을 둔다: testcore.ini, TestRust.ini
   ([Main User] URL·AccessKey·SecretKey, [Default] BucketName만 쓴다. 부하 값은 compare.sh가 덮어쓴다)
2. ./compare.sh --net-ini testcore.ini --rs-ini TestRust.ini
   (기본: 스레드 32, 60초, 1M, 3회. --threads/--times/--size/--files/--rounds로 바꿀 수 있다)
3. 결과: results/summary.md, results/runs.csv, results/logs/
"@ | Set-Content -Encoding utf8 (Join-Path $bundle "README.txt")

# 실행 권한을 주고 묶는다(Windows tar는 실행 비트를 남기지 않는다).
docker run --rm -v "${dist}:/dist" -w /dist alpine `
	sh -c "chmod +x perf-bundle/TestRust perf-bundle/compare.sh perf-bundle/testcore/TESTCore && tar -czf perf-bundle.tar.gz perf-bundle"
if ($LASTEXITCODE -ne 0) { throw "묶기 실패" }
Write-Output (Join-Path $dist "perf-bundle.tar.gz")
