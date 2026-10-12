# TestRust Linux 빌드·배포 스크립트 (TESTCore build.ps1·upload.ps1 흐름)
#
# rust:alpine 컨테이너(Docker Desktop 필요)에서 x86_64-unknown-linux-musl 정적 바이너리로 빌드한다. 필요한 라이브러리가
# 모두 들어 있어 대상 장비의 glibc 버전과 관계없이 x86_64 Linux에서 그대로 실행된다(Windows .exe는 Linux에서 쓸 수 없다).
#
#   [1/4] 이전 출력 정리   dist/linux/TestRust (빌드 캐시 target/linux는 남긴다)
#   [2/4] 빌드             버전은 TESTCore와 같은 `태그_커밋수_해시`
#   [3/4] 추가 파일 복사   sample.ini, controller.sample.ini, worker.sample.ini
#   [4/4] 압축             dist/linux/TestRust_<버전>.tar.gz (실행 권한 포함)
#   [배포]                 -Targets의 각 `user@host:/경로`로 올린다. 경로가 없으면 만들고, 실행 권한을 주고 --version으로 확인한다.
#                          config.ini 등 대상 장비에 있는 다른 파일은 건드리지 않는다.
#
# 사용법(저장소 루트에서):
#   pwsh ./build-linux.ps1                                  # 빌드 후 기본 대상에 배포
#   pwsh ./build-linux.ps1 -SkipDeploy                      # 빌드만
#   pwsh ./build-linux.ps1 -Targets root@192.168.31.103:/root/workspace/TestRust, root@192.168.31.104:/root/workspace/TestRust
param(
	[string]$OutputDir = (Join-Path $PSScriptRoot "dist\linux"),
	[string[]]$Targets = @(
		"root@192.168.11.156:/root/workspace/TestRust"
	),
	[switch]$SkipDeploy
)

$ErrorActionPreference = "Stop"

$ProjectName = "TestRust"
$ProjectRoot = $PSScriptRoot
$BinDir = Join-Path $OutputDir $ProjectName
$ExtraFiles = @("sample.ini", "controller.sample.ini", "worker.sample.ini")

Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "TestRust Linux 빌드 스크립트" -ForegroundColor Cyan
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host ""

# Step 1: Clean
Write-Host "[1/4] 이전 출력 정리 중..." -ForegroundColor Yellow
if (Test-Path $BinDir) {
	Remove-Item -Path $BinDir -Recurse -Force
	Write-Host "  - 삭제됨: $BinDir" -ForegroundColor Gray
}
Get-ChildItem -Path $OutputDir -Filter "${ProjectName}_*.tar.gz" -ErrorAction SilentlyContinue | ForEach-Object {
	Remove-Item -Path $_.FullName -Force
	Write-Host "  - 삭제됨: $($_.FullName)" -ForegroundColor Gray
}
New-Item -ItemType Directory -Force $BinDir | Out-Null
Write-Host "  정리 완료!" -ForegroundColor Green
Write-Host ""

# Step 2: Build (레지스트리 캐시는 이름 있는 볼륨, 빌드 결과는 Windows 빌드와 섞이지 않게 target/linux)
Write-Host "[2/4] Linux 정적 바이너리 빌드 중 (x86_64-unknown-linux-musl)..." -ForegroundColor Yellow
docker run --rm -v "${ProjectRoot}:/src" -v testrust-cargo-registry:/usr/local/cargo/registry -w /src rust:alpine `
	sh -c "apk add --no-cache musl-dev gcc git >/dev/null && git config --global --add safe.directory /src && cargo build --release -p testrust-cli --target-dir /src/target/linux"
if ($LASTEXITCODE -ne 0) {
	Write-Host "  빌드 실패!" -ForegroundColor Red
	exit $LASTEXITCODE
}
Copy-Item -Path (Join-Path $ProjectRoot "target\linux\release\$ProjectName") -Destination $BinDir -Force
Write-Host "  빌드 완료!" -ForegroundColor Green
Write-Host ""

# Step 3: Copy additional files
Write-Host "[3/4] 추가 파일 복사 중..." -ForegroundColor Yellow
foreach ($file in $ExtraFiles) {
	$source = Join-Path $ProjectRoot $file
	if (Test-Path $source) {
		Copy-Item -Path $source -Destination $BinDir -Force
		Write-Host "  - 복사됨: $file" -ForegroundColor Gray
	}
	else {
		Write-Host "  - 경고: $file 파일을 찾을 수 없습니다. 건너뜁니다..." -ForegroundColor Yellow
	}
}
Write-Host "  추가 파일 복사 완료!" -ForegroundColor Green
Write-Host ""

# Step 4: Compress (Windows tar는 실행 권한을 남기지 않으므로 컨테이너 안에서 권한을 주고 묶는다)
Write-Host "[4/4] tar.gz 압축 파일 생성 중..." -ForegroundColor Yellow
function Get-GitValue([string[]]$GitArgs, [string]$Default) {
	try {
		$value = (git -C $ProjectRoot @GitArgs 2>$null) -replace "`n", "" -replace "`r", ""
		if ([string]::IsNullOrEmpty($value)) { $Default } else { $value }
	}
	catch { $Default }
}
$GitTag = Get-GitValue @("describe", "--tags", "--abbrev=0") "v0.0.0"
$GitCommitCount = Get-GitValue @("rev-list", "--count", "HEAD") "0"
$GitShortHash = Get-GitValue @("rev-parse", "--short", "HEAD") "unknown"
$Version = "${GitTag}_${GitCommitCount}_${GitShortHash}"
$ArchiveName = "${ProjectName}_${Version}.tar.gz"
$ArchivePath = Join-Path $OutputDir $ArchiveName
Write-Host "  버전: $Version" -ForegroundColor Gray
Write-Host "  압축 파일: $ArchiveName" -ForegroundColor Gray
docker run --rm -v "${OutputDir}:/out" -w /out alpine `
	sh -c "chmod +x $ProjectName/$ProjectName && tar -czf $ArchiveName $ProjectName"
if ($LASTEXITCODE -ne 0) {
	Write-Host "  압축 실패!" -ForegroundColor Red
	exit $LASTEXITCODE
}
Write-Host "  압축 파일 생성 완료: $ArchivePath" -ForegroundColor Green
Write-Host ""

# Deploy
$Deployed = @()
if (-not $SkipDeploy) {
	Write-Host "[배포] 대상 $($Targets.Count)곳에 업로드 중..." -ForegroundColor Yellow
	foreach ($target in $Targets) {
		$hostPart, $dir = $target -split ":", 2
		if ([string]::IsNullOrEmpty($dir)) {
			Write-Host "  - 오류: '$target'은 user@host:/경로 형식이 아닙니다." -ForegroundColor Red
			exit 1
		}
		$dir = $dir.TrimEnd("/")
		ssh -n -o BatchMode=yes -o ConnectTimeout=10 $hostPart "mkdir -p '$dir'"
		if ($LASTEXITCODE -ne 0) {
			Write-Host "  - 실패: $target (원격 디렉터리 생성)" -ForegroundColor Red
			exit $LASTEXITCODE
		}
		$files = Get-ChildItem -Path $BinDir -File | ForEach-Object { $_.FullName }
		scp -q -o BatchMode=yes @files "${hostPart}:$dir/"
		if ($LASTEXITCODE -ne 0) {
			Write-Host "  - 실패: $target (업로드)" -ForegroundColor Red
			exit $LASTEXITCODE
		}
		$remoteVersion = ssh -n -o BatchMode=yes $hostPart "chmod +x '$dir/$ProjectName' && '$dir/$ProjectName' --version"
		if ($LASTEXITCODE -ne 0) {
			Write-Host "  - 실패: $target (실행 확인)" -ForegroundColor Red
			exit $LASTEXITCODE
		}
		Write-Host "  - upload $target done ($(($remoteVersion | Out-String).Trim()))" -ForegroundColor Gray
		$Deployed += $target
	}
	Write-Host "  배포 완료!" -ForegroundColor Green
	Write-Host ""
}

# Summary
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "빌드가 성공적으로 완료되었습니다!" -ForegroundColor Green
Write-Host "=====================================" -ForegroundColor Cyan
Write-Host "출력 디렉토리: $BinDir" -ForegroundColor White
Write-Host "압축 파일: $ArchivePath" -ForegroundColor White
foreach ($target in $Deployed) { Write-Host "배포: $target" -ForegroundColor White }
Write-Host ""
