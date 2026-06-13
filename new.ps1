# Utility script to flash a brand new ESP32-S3 with the latest WhisperEye firmwares
param (
    [Parameter(Mandatory = $false)]
    [switch]$Stable,

    [Parameter(Mandatory = $false)]
    [string]$Port = ""
)

$ErrorActionPreference = "Stop"

Clear-Host
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "   WhisperEye New ESP32-S3 Flash System                   " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# 1. Locate the correct production firmware
Write-Host "[*] Locating latest firmware binaries..." -ForegroundColor Gray
$BinFiles = Get-ChildItem "boards\board_default\firmware-s3-*.bin" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name

$SelectedBin = $null
$SelectedVersion = ""

if (-not $BinFiles) {
    Write-Error "No firmware binaries found in boards\board_default\"
}

$LatestMajor = 0; $LatestMinor = 0; $LatestPatch = 0; $LatestBuild = -1
$LatestIsStable = $false

foreach ($f in $BinFiles) {
    $maj = 0; $min = 0; $pat = 0; $bld = -1; $isStable = $false

    if ($f -match "firmware-s3-(\d+)\.(\d+)\.(\d+)-(\d+)\.bin") {
        $maj = [int]$Matches[1]; $min = [int]$Matches[2]; $pat = [int]$Matches[3]; $bld = [int]$Matches[4]
    } elseif ($f -match "firmware-s3-(\d+)\.(\d+)\.(\d+)\.bin") {
        $maj = [int]$Matches[1]; $min = [int]$Matches[2]; $pat = [int]$Matches[3]; $bld = 0; $isStable = $true
    } else {
        continue
    }

    if ($Stable -and -not $isStable) {
        continue # Skip unstable builds when looking for stable
    }

    # Comparison logic
    $isNewer = ($maj -gt $LatestMajor) -or
               ($maj -eq $LatestMajor -and $min -gt $LatestMinor) -or
               ($maj -eq $LatestMajor -and $min -eq $LatestMinor -and $pat -gt $LatestPatch) -or
               ($maj -eq $LatestMajor -and $min -eq $LatestMinor -and $pat -eq $LatestPatch -and $bld -gt $LatestBuild)

    if ($isNewer) {
        $LatestMajor = $maj; $LatestMinor = $min; $LatestPatch = $pat; $LatestBuild = $bld
        $LatestIsStable = $isStable
        $SelectedBin = "boards\board_default\$f"
        $SelectedVersion = if ($isStable) { "{0}.{1}.{2}" -f $maj,$min,$pat } else { "{0}.{1}.{2}-{3:D4}" -f $maj,$min,$pat,$bld }
    }
}

if (-not $SelectedBin) {
    Write-Error "Could not find any suitable firmware binary (Stable=$Stable)."
}

$RecoveryBin = "recovery_boot\recovery_boot.bin"
$OtaDataBin = "otadata_ota0.bin"

if (-not (Test-Path $RecoveryBin)) {
    Write-Error "Recovery binary not found at $RecoveryBin. Run run.ps1 once to compile and export it."
}
if (-not (Test-Path $OtaDataBin)) {
    Write-Error "OtaData binary not found at $OtaDataBin."
}

Write-Host "[+] Found Recovery Firmware: $RecoveryBin" -ForegroundColor Green
Write-Host "[+] Found Production Firmware: $SelectedBin ($SelectedVersion)" -ForegroundColor Green
Write-Host "[+] Found OtaData Image: $OtaDataBin" -ForegroundColor Green

# Setup ESP Toolchain environment variables if needed
$EspExportScript = "C:\Users\Alban\export-esp.ps1"
if (Test-Path $EspExportScript) {
    . $EspExportScript
}

# 2. Flash Bootloader, Partition Table, and Recovery App
# This is done by flashing recovery_boot which writes bootloader and partition table automatically
Write-Host "[*] [Step 1/3] Flashing bootloader, partition table and recovery partition..." -ForegroundColor Cyan
$Cmd1 = "cargo +esp espflash flash --flash-size 16mb --package recovery_boot --partition-table partitions.csv --target-app-partition recovery --after no-reset --release"
if ($Port) { $Cmd1 += " --port $Port" }
Write-Host "    -> Running: $Cmd1" -ForegroundColor DarkGray
Invoke-Expression $Cmd1

Write-Host "[*] Waiting 2 seconds for serial connection to settle..." -ForegroundColor Gray
Start-Sleep -Seconds 2

# 3. Flash OtaData binary at offset 0xf000
Write-Host "[*] [Step 2/3] Writing OTA Data image (otadata) at offset 0xf000..." -ForegroundColor Cyan
$Cmd2 = "cargo +esp espflash write-bin --before no-reset --after no-reset 0xf000 $OtaDataBin"
if ($Port) { $Cmd2 += " --port $Port" }
Write-Host "    -> Running: $Cmd2" -ForegroundColor DarkGray
Invoke-Expression $Cmd2

Write-Host "[*] Waiting 2 seconds for serial connection to settle..." -ForegroundColor Gray
Start-Sleep -Seconds 2

# 4. Flash Production app binary at offset 0x212000
Write-Host "[*] [Step 3/3] Writing Production App image (production) at offset 0x212000..." -ForegroundColor Cyan
$Cmd3 = "cargo +esp espflash write-bin --before no-reset --monitor 0x212000 $SelectedBin"
if ($Port) { $Cmd3 += " --port $Port" }
Write-Host "    -> Running: $Cmd3" -ForegroundColor DarkGray
Invoke-Expression $Cmd3

Write-Host "[+] Flash complete!" -ForegroundColor Green
