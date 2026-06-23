# PowerShell script to flash a complete ESP32-S3 firmware package (Factory / Out-of-box)
# This script does not require compilation tools (Cargo/Rust/etc.) and runs stand-alone.
param (
    [Parameter(Mandatory = $false)]
    [string]$Port = "",

    [Parameter(Mandatory = $false)]
    [int]$Baud = 460800
)

$ErrorActionPreference = "Stop"

Clear-Host
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "   WhisperEye Standalone ESP32-S3 Factory Flash Utility   " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

$BinDir = $PSScriptRoot
if (-not $BinDir) { $BinDir = "." }

# List of required files
$RequiredFiles = @{
    "bootloader.bin" = "0x0"
    "partitions.bin" = "0x8000"
    "nvs.bin"        = "0x9000"
    "otadata.bin"    = "0xf000"
    "phy_init.bin"   = "0x11000"
    "recovery.bin"   = "0x20000"
    "production.bin" = "0x220000"
}

# 1. Verify that all binary files exist in the same directory
Write-Host "[*] Checking for required firmware binaries..." -ForegroundColor Gray
$MissingFiles = @()
foreach ($file in $RequiredFiles.Keys) {
    $FilePath = Join-Path $BinDir $file
    if (-not (Test-Path $FilePath)) {
        $MissingFiles += $file
    }
}

if ($MissingFiles.Count -gt 0) {
    Write-Host "[-] Error: Missing required binary files in the flash folder:" -ForegroundColor Red
    foreach ($file in $MissingFiles) {
        Write-Host "  -> $file" -ForegroundColor Yellow
    }
    Write-Host "[!] Run WhisperEye\run.ps1 compilation first to generate them." -ForegroundColor Yellow
    exit 1
}
Write-Host "[+] All required firmware binaries are present." -ForegroundColor Green

# 2. Check for esptool.exe and download it if missing
$EsptoolExe = Join-Path $BinDir "esptool.exe"

# If not at root, but inside a nested folder (e.g. esptool-win64), move it first!
$NestedFolder = Get-ChildItem $BinDir -Directory | Where-Object { $_.Name -like "esptool-*" } | Select-Object -First 1
if ($NestedFolder -and -not (Test-Path $EsptoolExe)) {
    Write-Host "[*] esptool.exe found in nested folder, moving to root..." -ForegroundColor Gray
    Get-ChildItem $NestedFolder.FullName | Move-Item -Destination $BinDir -Force
    Remove-Item $NestedFolder.FullName -Recurse -Force
}

if (-not (Test-Path $EsptoolExe)) {
    Write-Host "[*] esptool.exe not found. Downloading officially from Espressif GitHub releases..." -ForegroundColor Cyan
    $Url = "https://github.com/espressif/esptool/releases/download/v4.8.1/esptool-v4.8.1-win64.zip"
    $ZipPath = Join-Path $BinDir "esptool.zip"
    
    try {
        Write-Host "    -> Downloading Zip from: $Url" -ForegroundColor DarkGray
        Invoke-WebRequest -Uri $Url -OutFile $ZipPath -UseBasicParsing
        
        Write-Host "    -> Extracting esptool.exe..." -ForegroundColor DarkGray
        Expand-Archive -Path $ZipPath -DestinationPath $BinDir -Force
        
        # Cleanup zip file
        Remove-Item $ZipPath -ErrorAction SilentlyContinue
        
        # Handle folder nesting inside zip (esptool zip contains a folder starting with esptool-)
        $NestedFolder = Get-ChildItem $BinDir -Directory | Where-Object { $_.Name -like "esptool-*" } | Select-Object -First 1
        if ($NestedFolder) {
            Get-ChildItem $NestedFolder.FullName | Move-Item -Destination $BinDir -Force
            Remove-Item $NestedFolder.FullName -Recurse -Force
        }
        Write-Host "[+] esptool.exe downloaded and extracted successfully." -ForegroundColor Green
    }
    catch {
        Write-Host "[-] Failed to download/extract esptool.exe!" -ForegroundColor Red
        Write-Host "    Error detail: $_" -ForegroundColor DarkGray
        Write-Host "[!] Please download esptool.exe manually and place it in this directory: $BinDir" -ForegroundColor Yellow
        exit 1
    }
}

# 3. Detect COM Port if not specified
if (-not $Port) {
    Write-Host "[*] Auto-detecting COM ports..." -ForegroundColor Gray
    try {
        $cimPorts = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue | 
        Where-Object { $_.Caption -match 'COM\d+' -and $_.Caption -notmatch 'Bluetooth' }
        
        if (-not $cimPorts) {
            Write-Host "[-] No COM ports detected!" -ForegroundColor Red
            Write-Host "    Connect your ESP32-S3 and check USB drivers (CP210x / CH34x / USB JTAG)." -ForegroundColor Yellow
            exit 1
        }
        
        $PortCount = 0
        $LastPort = ""
        foreach ($dev in $cimPorts) {
            if ($dev.Caption -match '(COM\d+)') {
                $LastPort = $Matches[1]
                Write-Host "  -> Found: $($dev.Caption)" -ForegroundColor Green
                $PortCount++
            }
        }
        
        if ($PortCount -eq 1) {
            $Port = $LastPort
            Write-Host "[+] Auto-selected single port: $Port" -ForegroundColor Green
        }
        else {
            Write-Host "[!] Multiple COM ports found. Please specify the target port." -ForegroundColor Yellow
            $PortInput = Read-Host "Enter COM port (e.g. COM3)"
            if ($PortInput -match '^COM\d+$') {
                $Port = $PortInput
            }
            else {
                Write-Host "[-] Invalid COM port format!" -ForegroundColor Red
                exit 1
            }
        }
    }
    catch {
        Write-Host "[!] Port detection failed, relying on esptool auto-select." -ForegroundColor Yellow
    }
}

# 4. Construct and execute the esptool command
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "   FLASHING TARGET DEVICE ON PORT: $Port (Baud: $Baud)" -ForegroundColor Yellow
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "[*] Executing esptool.exe..." -ForegroundColor Gray

$FlashArgs = @(
    "--chip", "esp32s3"
)

if ($Port) {
    $FlashArgs += "--port"
    $FlashArgs += $Port
}

$FlashArgs += @(
    "--baud", [string]$Baud,
    "write_flash", "-z"
)

# Add binary files and offsets to arguments list
foreach ($file in $RequiredFiles.Keys) {
    $FlashArgs += $RequiredFiles[$file]
    $FlashArgs += (Join-Path $BinDir $file)
}

# Print command for transparency
$PrintCmd = "esptool.exe " + ($FlashArgs -join " ")
Write-Host "Command: $PrintCmd" -ForegroundColor DarkGray
Write-Host ""

$Success = $true
try {
    # Run esptool.exe directly to pipe output/logs in real-time into the terminal
    & $EsptoolExe @FlashArgs
    if ($LASTEXITCODE -ne 0) {
        $Success = $false
    }
}
catch {
    Write-Host "[-] Execution failed with error: $_" -ForegroundColor Red
    $Success = $false
}

# 5. Handle success/failure
Write-Host ""
if ($Success) {
    Write-Host "==========================================================" -ForegroundColor Green
    Write-Host "       ESP32-S3 FIRMWARE FLASHED SUCCESSFULLY!            " -ForegroundColor Green
    Write-Host "==========================================================" -ForegroundColor Green
    Write-Host "You can now reset your device to boot into the new firmware." -ForegroundColor Gray
}
else {
    Write-Host "==========================================================" -ForegroundColor Red
    Write-Host "              FLASHING / UPLOAD FAILED!                   " -ForegroundColor Red
    Write-Host "==========================================================" -ForegroundColor Red
    Write-Host "Suggestions:" -ForegroundColor Yellow
    Write-Host "  1. Ensure the board is in Bootloader mode (Hold BOOT button, press RST/EN, release BOOT)." -ForegroundColor Gray
    Write-Host "  2. Check USB cables and connection." -ForegroundColor Gray
    Write-Host "  3. Try lowering the baudrate, e.g.: .\flash.ps1 -Port $Port -Baud 115200" -ForegroundColor Gray
    exit 1
}
