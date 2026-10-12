# TESTCore 커밋(기본 HEAD) 소스를 임시 디렉터리로 내보내(git archive, 읽기 전용) 그곳에서 빌드한다.
# TESTCore 저장소의 bin/는 예전 커밋으로 빌드된 것일 수 있으므로 기준 출력은 이 빌드로 만든다.
#
# 사용법(저장소 루트에서):
#   $bin = pwsh tools/dotnet-oracle/build-testcore.ps1
#   dotnet build tools/dotnet-oracle -p:TestCoreBin=$bin
#   $env:TESTCORE_BIN = $bin
#   pwsh tools/dotnet-oracle/build-testcore.ps1 -Ref dotnet-final   # 전환 시점의 .NET 기준 구현
param(
	[string]$TestCore = "E:\Code\Git\TESTCore",
	[string]$Ref = "HEAD",
	[string]$Out = (Join-Path ([IO.Path]::GetTempPath()) "testrust-testcore-head")
)
$ErrorActionPreference = "Stop"
$commit = git -C $TestCore rev-parse --short "$Ref^{commit}"
$src = Join-Path $Out $commit
$bin = Join-Path $src "bin\TestCore"
if (-not (Test-Path (Join-Path $bin "TestCore.dll"))) {
	if (Test-Path $src) { Remove-Item -Recurse -Force $src }
	New-Item -ItemType Directory -Force $src | Out-Null
	$tar = Join-Path $Out "$commit.tar"
	git -C $TestCore archive --format=tar $Ref -o $tar
	tar -xf $tar -C $src
	Remove-Item $tar
	dotnet build (Join-Path $src "TESTCore.csproj") -c Release -o $bin -v q -nologo | Out-Host
}
$bin
