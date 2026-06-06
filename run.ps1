# PowerShell Utility script for WhisperEye Workspace compilation and upload
param (
    [Parameter(Mandatory = $false)]
    [ValidateSet("recovery", "production", "nvs", "all")]
    [string]$Target = "all",

    [Parameter(Mandatory = $false)]
    [string]$Port = "",

    [Parameter(Mandatory = $false)]
    [switch]$Clean
)

$ErrorActionPreference = "Stop"

# Clear host and print a premium welcome header
Clear-Host
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "   WhisperEye Firmware Build & Flash Automation System   " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# 1. Setup Environment
Write-Host "[*] Configuring ESP Toolchain and Cargo environment..." -ForegroundColor Gray
$env:CARGO_TARGET_DIR = "C:\t\we"
$env:LDPROXY_LINKER = "xtensa-esp32s3-elf-gcc"
Write-Host "    -> Target Directory set to: $env:CARGO_TARGET_DIR (bypassing Windows path limits)" -ForegroundColor DarkGray
Write-Host "    -> LDProxy Linker set to: $env:LDPROXY_LINKER" -ForegroundColor DarkGray

$EspExportScript = "C:\Users\Alban\export-esp.ps1"
if (Test-Path $EspExportScript) {
    Write-Host "    -> Sourcing Espressif Toolchain: $EspExportScript..." -ForegroundColor DarkGray
    . $EspExportScript
}
else {
    Write-Host "    [!] Warning: Espressif environment script not found at standard path. Relying on PATH variables." -ForegroundColor Yellow
}

# Helper functions for versioning and catalog update
function Increment-ProductionVersion {
    # Pattern: firmware-s3-1.0.1-0125.bin  =>  base=1.0.1, build=125
    $BinFiles = Get-ChildItem "boards\board_default\firmware-s3-*.bin" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
    $BaseVersion = "1.0.1"
    $HighestBuild = 0

    if ($BinFiles) {
        foreach ($f in $BinFiles) {
            if ($f -match "firmware-s3-(\d+\.\d+\.\d+)-(\d+)\.bin") {
                $build = [int]$Matches[2]
                if ($build -gt $HighestBuild) {
                    $HighestBuild = $build
                    $BaseVersion = $Matches[1]
                }
            }
        }
    }

    $NewBuild = $HighestBuild + 1
    $NewVersion = "{0}-{1:D4}" -f $BaseVersion, $NewBuild
    Write-Host "[+] New WhisperEye build version: $NewVersion" -ForegroundColor Green

    # Update FW_VERSION constant in production_app/src/main.rs
    $MainRsPath = "production_app\src\main.rs"
    if (Test-Path $MainRsPath) {
        $MainRsContent = Get-Content $MainRsPath -Raw
        $Pattern = 'const FW_VERSION:\s*&str\s*=\s*"[^"]*";'
        $Replacement = 'const FW_VERSION: &str = "' + $NewVersion + '";'
        if ($MainRsContent -match $Pattern) {
            $MainRsContent = $MainRsContent -replace $Pattern, $Replacement
            Set-Content $MainRsPath $MainRsContent
            Write-Host "    [+] Updated FW_VERSION to '$NewVersion' in $MainRsPath" -ForegroundColor DarkGray
        } else {
            Write-Host "    [!] Warning: Could not find const FW_VERSION in $MainRsPath" -ForegroundColor Yellow
        }
    }

    return $NewVersion
}

function Increment-RecoveryVersion {
    $RecoveryRsPath = "recovery_boot\src\main.rs"
    if (Test-Path $RecoveryRsPath) {
        $RsContent = Get-Content $RecoveryRsPath -Raw
        $Pattern = 'const FW_VERSION:\s*&str\s*=\s*"([^"]*)";'
        if ($RsContent -match $Pattern) {
            $CurrentVersion = $Matches[1]
            if ($CurrentVersion -match "(.+)-(\d+)$") {
                $Base = $Matches[1]
                $Build = [int]$Matches[2] + 1
                $NewVersion = "{0}-{1:D4}" -f $Base, $Build
            } else {
                $NewVersion = $CurrentVersion + "-0001"
            }
            $Replacement = 'const FW_VERSION: &str = "' + $NewVersion + '";'
            $RsContent = $RsContent -replace $Pattern, $Replacement
            Set-Content $RecoveryRsPath $RsContent
            Write-Host "[+] Updated recovery FW_VERSION to '$NewVersion' in $RecoveryRsPath" -ForegroundColor Green
            return $NewVersion
        } else {
            Write-Host "    [!] Warning: Could not find const FW_VERSION in $RecoveryRsPath" -ForegroundColor Yellow
        }
    }
    return "1.0.0-recovery"
}


function Update-FirmwareJson {
    param([string]$NewVersion)
    $JsonPath = "boards\board_default\firmware.json"
    if (-not (Test-Path $JsonPath)) {
        Write-Host "    [!] Warning: firmware.json not found at $JsonPath" -ForegroundColor Yellow
        return
    }

    $JsonContent = Get-Content $JsonPath -Raw | ConvertFrom-Json

    foreach ($board in $JsonContent) {
        $chipType = $board.ChipType

        # Only update the ESP32-S3 / S3 entries automatically (the chip we just compiled)
        if ($chipType -ne "ESP32-S3") { continue }

        $prefix = "firmware-s3-"
        $pattern = "$prefix*.bin"
        $files = Get-ChildItem "boards\board_default\$pattern" -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name
        if (-not $files) { continue }

        # Collect all builds from disk (format: 1.0.1-0125)
        $allBuilds = @()
        foreach ($file in $files) {
            if ($file -match "$prefix(\d+\.\d+\.\d+)-(\d+)\.bin") {
                $verStr = $Matches[1] + "-" + $Matches[2]
                $buildNum = [int]$Matches[2]
                $url = "https://github.com/sctfic/WhisperEye/raw/main/boards/board_default/$file"
                $allBuilds += [PSCustomObject]@{ version = $verStr; url = $url; build = $buildNum }
            }
        }

        # Sort descending by build number, keep only 2 most recent as unstable
        $sortedUnstable = $allBuilds | Sort-Object build -Descending | Select-Object -First 2 | ForEach-Object {
            [PSCustomObject]@{ version = $_.version; url = $_.url }
        }

        # Preserve stable as-is (user manages stable entries manually)
        $board.unstable = if ($sortedUnstable) { @($sortedUnstable) } else { @() }
    }

    $updatedJson = $JsonContent | ConvertTo-Json -Depth 4
    Set-Content $JsonPath $updatedJson
    Write-Host "    [+] boards/board_default/firmware.json successfully updated!" -ForegroundColor Green
}


# 2. Setup targets
if ($Target -eq "nvs") {
    Write-Host "[*] Initiating NVS erase process..." -ForegroundColor Cyan
    $FlashCommand = "cargo +esp espflash erase-parts --package recovery_boot --partition-table partitions.csv nvs"
    if ($Port) {
        $FlashCommand += " --port $Port"
    }
    Write-Host "    -> Invoking command: $FlashCommand" -ForegroundColor DarkGray
    try {
        Invoke-Expression $FlashCommand
    }
    catch {
        # Failures handled below
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] NVS erase failed!" -ForegroundColor Red
        exit 1
    }
    Write-Host "[+] NVS erase successful!" -ForegroundColor Green
    exit 0
}

if ($Target -eq "all") {
    Write-Host "[*] Initiating build and flash for ALL targets (recovery & production)..." -ForegroundColor Cyan
    
    # 3. Clean Cache (if requested)
    if ($Clean) {
        Write-Host "[*] Cleaning Cargo cache..." -ForegroundColor Gray
        cargo +esp clean
    }

    # Increment recovery version before compile
    $NewRecoveryVersion = Increment-RecoveryVersion

    # Compilation 1/2: recovery_boot
    $BuildProfile = if ($Debug) { "debug" } else { "release" }
    Write-Host "[*] [1/2] Compiling recovery_boot ($BuildProfile)..." -ForegroundColor Cyan
    if ($Debug) {
        cargo +esp build --package recovery_boot
    } else {
        cargo +esp build --package recovery_boot --release
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Compilation of recovery_boot failed!" -ForegroundColor Red
        exit 1
    }

    # Export recovery boot bin image (without build number, named recovery_boot.bin)
    $RecoveryBinPath = "recovery_boot\recovery_boot.bin"
    Write-Host "[*] Exporting recovery binary image to $RecoveryBinPath..." -ForegroundColor Cyan
    $SaveRecoveryCmd = "cargo +esp espflash save-image --chip esp32s3 --flash-size 16mb --package recovery_boot --partition-table partitions.csv --target-app-partition recovery"
    if ($BuildProfile -eq "release") {
        $SaveRecoveryCmd += " --release"
    }
    $SaveRecoveryCmd += " $RecoveryBinPath"
    Write-Host "    -> Invoking command: $SaveRecoveryCmd" -ForegroundColor DarkGray
    Invoke-Expression $SaveRecoveryCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Failed to save ESP32-S3 recovery binary image!" -ForegroundColor Red
        exit 1
    }

    # Automated version incrementation and packaging pipeline
    $NewVersion = Increment-ProductionVersion

    # Compilation 2/2: production_app
    Write-Host "[*] [2/2] Compiling production_app ($BuildProfile)..." -ForegroundColor Cyan
    if ($Debug) {
        cargo +esp build --package production_app
    } else {
        cargo +esp build --package production_app --release
    }
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Compilation of production_app failed!" -ForegroundColor Red
        exit 1
    }
    
    $BinPath = "boards\board_default\firmware-s3-$NewVersion.bin"
    Write-Host "[*] Exporting flashable binary image to $BinPath..." -ForegroundColor Cyan
    $SaveCmd = "cargo +esp espflash save-image --chip esp32s3 --flash-size 16mb --package production_app --partition-table partitions.csv --target-app-partition production"
    if ($BuildProfile -eq "release") {
        $SaveCmd += " --release"
    }
    $SaveCmd += " $BinPath"
    Write-Host "    -> Invoking command: $SaveCmd" -ForegroundColor DarkGray
    Invoke-Expression $SaveCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Failed to save ESP32-S3 binary image!" -ForegroundColor Red
        exit 1
    }
    Update-FirmwareJson -NewVersion $NewVersion

    Write-Host "[+] All target packages compiled successfully. Ready to flash." -ForegroundColor Green

    # Flashing 1/2: recovery_boot (NO monitor, keep in bootloader)
    Write-Host "[*] [1/2] Flashing recovery_boot onto 'recovery' partition (keeping bootloader active)..." -ForegroundColor Cyan
    $FlashRecovery = "cargo +esp espflash flash --flash-size 16mb --package recovery_boot --partition-table partitions.csv --target-app-partition recovery --after no-reset"
    if (-not $Debug) { $FlashRecovery += " --release" }
    if ($Port) { $FlashRecovery += " --port $Port" }
    Write-Host "    -> Invoking command: $FlashRecovery" -ForegroundColor DarkGray
    try {
        Invoke-Expression $FlashRecovery
    } catch {}
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Flashing of recovery_boot failed!" -ForegroundColor Red
        exit 1
    }

    # Short delay to allow serial port driver to settle under Windows
    Write-Host "[*] Waiting 2 seconds for serial port driver to settle..." -ForegroundColor Gray
    Start-Sleep -Seconds 2

    # Flashing 1.5/2: otadata (to point boot to production/ota_0)
    Write-Host "[*] [1.5/2] Flashing otadata_ota0.bin onto 'otadata' partition..." -ForegroundColor Cyan
    $FlashOtaData = "cargo +esp espflash write-bin --before no-reset --after no-reset 0xf000 otadata_ota0.bin"
    if ($Port) { $FlashOtaData += " --port $Port" }
    Write-Host "    -> Invoking command: $FlashOtaData" -ForegroundColor DarkGray
    try {
        Invoke-Expression $FlashOtaData
    } catch {}
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Flashing of otadata failed!" -ForegroundColor Red
        exit 1
    }

    # Flashing 2/2: production_app (WITH monitor)
    Write-Host "[*] [2/2] Flashing production_app onto 'production' partition..." -ForegroundColor Cyan
    $FlashProd = "cargo +esp espflash flash --flash-size 16mb --package production_app --partition-table partitions.csv --target-app-partition production --before no-reset --monitor"
    if (-not $Debug) { $FlashProd += " --release" }
    if ($Port) { $FlashProd += " --port $Port" }
    Write-Host "    -> Invoking command: $FlashProd" -ForegroundColor DarkGray
    try {
        Invoke-Expression $FlashProd
    } catch {}
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Flashing of production_app failed!" -ForegroundColor Red
        exit 1
    }
    Write-Host "[+] All builds and flashing completed successfully!" -ForegroundColor Green
    exit 0
}

$Package = if ($Target -eq "recovery") { "recovery_boot" } else { "production_app" }
$BuildProfile = if ($Debug) { "debug" } else { "release" }
$ProfileFlag = if ($Debug) { "" } else { "--release" }

# 3. Clean Cache
if ($Clean) {
    Write-Host "[*] Cleaning Cargo cache..." -ForegroundColor Gray
    cargo +esp clean
}

# 4. Compile
if ($Package -eq "production_app") {
    $NewVersion = Increment-ProductionVersion
} elseif ($Package -eq "recovery_boot") {
    $NewRecoveryVersion = Increment-RecoveryVersion
}
Write-Host "[*] Compiling target package: $Package ($BuildProfile)..." -ForegroundColor Cyan
if ($Debug) {
    cargo +esp build --package $Package
}
else {
    cargo +esp build --package $Package --release
}

if ($LASTEXITCODE -ne 0) {
    Write-Host "[-] Compilation failed!" -ForegroundColor Red
    exit 1
}
Write-Host "[+] Compilation successful!" -ForegroundColor Green

if ($Package -eq "production_app") {
    $BinPath = "boards\board_default\firmware-s3-$NewVersion.bin"
    Write-Host "[*] Exporting flashable binary image to $BinPath..." -ForegroundColor Cyan
    $SaveCmd = "cargo +esp espflash save-image --chip esp32s3 --flash-size 16mb --package production_app --partition-table partitions.csv --target-app-partition production"
    if ($BuildProfile -eq "release") {
        $SaveCmd += " --release"
    }
    $SaveCmd += " $BinPath"
    Write-Host "    -> Invoking command: $SaveCmd" -ForegroundColor DarkGray
    Invoke-Expression $SaveCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Failed to save ESP32-S3 binary image!" -ForegroundColor Red
        exit 1
    }
    Update-FirmwareJson -NewVersion $NewVersion
} elseif ($Package -eq "recovery_boot") {
    $RecoveryBinPath = "recovery_boot\recovery_boot.bin"
    Write-Host "[*] Exporting recovery binary image to $RecoveryBinPath..." -ForegroundColor Cyan
    $SaveRecoveryCmd = "cargo +esp espflash save-image --chip esp32s3 --flash-size 16mb --package recovery_boot --partition-table partitions.csv --target-app-partition recovery"
    if ($BuildProfile -eq "release") {
        $SaveRecoveryCmd += " --release"
    }
    $SaveRecoveryCmd += " $RecoveryBinPath"
    Write-Host "    -> Invoking command: $SaveRecoveryCmd" -ForegroundColor DarkGray
    Invoke-Expression $SaveRecoveryCmd
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Failed to save ESP32-S3 recovery binary image!" -ForegroundColor Red
        exit 1
    }
}


# 5. Flash Upload
Write-Host "[*] Initiating upload process for $Package..." -ForegroundColor Cyan

if ($Target -eq "recovery") {
    # Erase otadata so bootloader defaults to recovery boot
    Write-Host "[*] Erasing 'otadata' partition to default to recovery boot..." -ForegroundColor Cyan
    $EraseOtaData = "cargo +esp espflash erase-parts --partition-table partitions.csv --after no-reset otadata"
    if ($Port) { $EraseOtaData += " --port $Port" }
    Write-Host "    -> Invoking command: $EraseOtaData" -ForegroundColor DarkGray
    try {
        Invoke-Expression $EraseOtaData
    } catch {}
    if ($LASTEXITCODE -ne 0) {
        Write-Host "[-] Erasing of otadata failed!" -ForegroundColor Red
        exit 1
    }
}

# Flash the firmware image
$FlashCommand = "cargo +esp espflash flash --flash-size 16mb --package $Package --partition-table partitions.csv"
if ($Target -eq "production") {
    $FlashCommand += " --target-app-partition production"
}
else {
    $FlashCommand += " --target-app-partition recovery --before no-reset"
}
if (-not $Debug) {
    $FlashCommand += " --release"
}
if ($Port) {
    $FlashCommand += " --port $Port"
}
# For production: no --monitor here, we still need to flash otadata after
# For recovery: attach monitor directly
if ($Target -ne "production") {
    $FlashCommand += " --monitor"
}

Write-Host "    -> Invoking command: $FlashCommand" -ForegroundColor DarkGray

try {
    Invoke-Expression $FlashCommand
}
catch {
    # Failures are handled below
}

if ($LASTEXITCODE -ne 0) {
    Write-Host "[-] Flashing of $Package failed!" -ForegroundColor Red
    exit 1
}

if ($Target -eq "production") {
    # Flash otadata AFTER the production firmware to point boot to ota_0
    Write-Host "[*] Waiting 2 seconds for serial port driver to settle..." -ForegroundColor Gray
    Start-Sleep -Seconds 2

    Write-Host "[*] Flashing otadata_ota0.bin onto 'otadata' partition and starting monitor..." -ForegroundColor Cyan
    $FlashOtaData = "cargo +esp espflash write-bin 0xf000 otadata_ota0.bin --monitor"
    if ($Port) { $FlashOtaData += " --port $Port" }
    Write-Host "    -> Invoking command: $FlashOtaData" -ForegroundColor DarkGray
    try {
        Invoke-Expression $FlashOtaData
    } catch {}
}


# Check if execution failed
if ($LASTEXITCODE -ne 0) {
    Write-Host ""
    Write-Host "==========================================================" -ForegroundColor Red
    Write-Host "               UPLOAD / FLASHING FAILED!                  " -ForegroundColor Red
    Write-Host "==========================================================" -ForegroundColor Red
    Write-Host ""
    
    # - Target Configuration
    Write-Host "--- 1. CONFIGURATION DE LA CIBLE ---" -ForegroundColor Yellow
    Write-Host "• Microcontrôleur Cible : ESP32-S3-N16R8" -ForegroundColor Gray
    Write-Host "• Target Rust           : xtensa-esp32s3-espidf" -ForegroundColor Gray
    Write-Host "• Package sélectionné   : $Package" -ForegroundColor Gray
    Write-Host "• Fichier de partitions : partitions.csv (Recovery: 2MB, Production: 14.2MB)" -ForegroundColor Gray
    Write-Host ""

    # - Parametres de Flash / Monitor
    Write-Host "--- 2. PARAMÈTRES DE FLASH / MONITEUR ---" -ForegroundColor Yellow
    if ($Port) {
        Write-Host "• Port Série Spécifié   : $Port" -ForegroundColor Gray
    }
    else {
        Write-Host "• Port Série Spécifié   : Automatique (Détection espflash)" -ForegroundColor Gray
    }
    Write-Host "• Vitesse de Flash      : 460800 bauds (Standard)" -ForegroundColor Gray
    Write-Host "• Terminal Moniteur     : Actif (--monitor)" -ForegroundColor Gray
    Write-Host ""

    # - Ports serie (COM) detectes sur le systeme
    Write-Host "--- 3. PORTS SÉRIE (COM) DÉTECTÉS ---" -ForegroundColor Yellow
    try {
        $cimPorts = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue | 
        Where-Object { $_.Caption -match 'COM\d+' -and $_.Caption -notmatch 'Bluetooth' }
        if ($cimPorts) {
            foreach ($dev in $cimPorts) {
                Write-Host "  [OK] $($dev.Caption)" -ForegroundColor Green
            }
        }
        else {
            Write-Host "[-] Aucun port COM n'a été détecté !" -ForegroundColor Red
            Write-Host "    Veuillez vérifier vos branchements USB et vos pilotes (CP210x / CH34x)." -ForegroundColor Gray
        }
    }
    catch {
        Write-Host "[-] Erreur lors du listing des ports COM." -ForegroundColor Red
    }
    Write-Host ""

    # - Procedure du Bootloader
    Write-Host "--- 4. PROCÉDURE DU BOOTLOADER MATÉRIEL ---" -ForegroundColor Yellow
    Write-Host "Si le téléversement automatique échoue, forcez le mode Bootloader matériel :" -ForegroundColor Gray
    Write-Host "  1. Maintenez le bouton [BOOT] (GPIO 0) enfoncé sur la carte." -ForegroundColor Cyan
    Write-Host "  2. Appuyez brièvement sur le bouton [EN / RST] (Reset)." -ForegroundColor Cyan
    Write-Host "  3. Relâchez le bouton [BOOT]." -ForegroundColor Cyan
    Write-Host "  4. Relancez immédiatement la commande d'upload." -ForegroundColor Cyan
    Write-Host "==========================================================" -ForegroundColor Red
    exit 1
}
else {
    Write-Host "[+] Upload and monitor session exited cleanly." -ForegroundColor Green
}
