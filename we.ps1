<#
.SYNOPSIS
    WhisperEye Helper Tool - Compile, Flash, and Monitor ESP32-S3 firmware in a single command.
.DESCRIPTION
    This script automates loading the ESP Xtensa toolchain environment, setting up target directory
    optimizations, and running cargo espflash with monitor mode.
.PARAMETER Release
    Switch to compile in release mode (highly recommended for production/size).
.PARAMETER Clean
    Switch to clean the cargo target folder before building.
.EXAMPLE
    .\we.ps1
.EXAMPLE
    .\we.ps1 -Release
.EXAMPLE
    .\we.ps1 -Clean
#>

[CmdletBinding()]
param(
    [switch]$Release,
    [switch]$Clean,
    [switch]$OtaBase,
    [switch]$Build,
    [Parameter(ValueFromRemainingArguments=$true)]
    $RemainingArgs
)

Write-Host "`n[WhisperEye] - Utility Tool" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Gray

# 0. Libérer les verrous de fichiers Windows (fermer les instances concurrentes de cargo/rustc/espflash/rust-analyzer)
try {
    Stop-Process -Name cargo, rustc, espflash, rust-analyzer -ErrorAction SilentlyContinue
} catch {}

# 1. Ensure Cargo bin directory is in PATH
$cargoBin = "C:\Users\Alban\.cargo\bin"
if (Test-Path $cargoBin) {
    if ($Env:PATH -notlike "*$cargoBin*") {
        $Env:PATH = "$cargoBin;" + $Env:PATH
    }
}

# 2. Load Xtensa Toolchain
$exportPath = "C:\Users\Alban\export-esp.ps1"
if (Test-Path $exportPath) {
    Write-Host "[*] Loading ESP toolchain from $exportPath..." -ForegroundColor Yellow
    . $exportPath
} else {
    Write-Warning "ESP toolchain script not found at $exportPath. Assuming tools are already on PATH."
}

# 3. Optimize Target Directory for Windows (removes path length limits and speeds up build)
Write-Host "[*] Setting Cargo target directory to 'C:\t\we' (bypassing Windows path length limits)..." -ForegroundColor Yellow
$env:CARGO_TARGET_DIR = "C:\t\we"

# 4. Change directory dynamically based on selected target
if ($OtaBase) {
    $boardPath = Join-Path $PSScriptRoot "boards\ota_base"
    Write-Host "[*] Option -OtaBase selectionnee. Cible : WhisperEye-OTA-Base-V1" -ForegroundColor Yellow
    # La partition ota_0 étant limitée à 1MB, ota_base doit impérativement être compilé en mode Release pour tenir.
    $Release = $true
} else {
    $boardPath = Join-Path $PSScriptRoot "boards\board_default"
    Write-Host "[*] Cible par defaut selectionnee : WhisperEye-Full-Last" -ForegroundColor Yellow
}
Push-Location $boardPath

function Show-Diagnostics {
    param(
        [string]$SelectedPort,
        [string]$BaudRate
    )

    Write-Host "`n--- DIAGNOSTICS & CONFIGURATION ---" -ForegroundColor Yellow
    Write-Host "----------------------------------------" -ForegroundColor Gray

    # 1. Target Configuration
    $mcu = "esp32"
    $target = "xtensa-esp32-espidf"
    $version = "v5.2.1"
    
    $configPath = Join-Path $boardPath ".cargo\config.toml"
    if (Test-Path $configPath) {
        $content = Get-Content $configPath
        foreach ($line in $content) {
            if ($line -match 'MCU\s*=\s*"([^"]+)"') { $mcu = $Matches[1] }
            elseif ($line -match 'target\s*=\s*"([^"]+)"') { $target = $Matches[1] }
            elseif ($line -match 'ESP_IDF_VERSION\s*=\s*"([^"]+)"') { $version = $Matches[1] }
        }
    }

    Write-Host "Target Configuration (depuis $boardPath\.cargo\config.toml) :" -ForegroundColor White
    Write-Host "  - MCU Target      : $mcu" -ForegroundColor Cyan
    Write-Host "  - Rust Target     : $target" -ForegroundColor Cyan
    Write-Host "  - ESP-IDF Version : $version" -ForegroundColor Cyan

    # 2. Port & Baudrate
    Write-Host "`nParametres de Flash / Monitor :" -ForegroundColor White
    if ($SelectedPort) {
        Write-Host "  - Port specifie   : $SelectedPort" -ForegroundColor Cyan
    } else {
        Write-Host "  - Port specifie   : Auto-detecte (espflash)" -ForegroundColor Cyan
    }
    if ($BaudRate) {
        Write-Host "  - Debit (Baud)    : $BaudRate" -ForegroundColor Cyan
    } else {
        Write-Host "  - Debit (Baud)    : Par defaut (espflash auto)" -ForegroundColor Cyan
    }

    # 3. Available Serial Ports
    Write-Host "`nPorts serie (COM) detectes sur le systeme :" -ForegroundColor White
    $portsFound = $false
    try {
        $cimPorts = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue | 
            Where-Object { $_.Caption -match 'COM\d+' -and $_.Caption -notmatch 'Bluetooth' }
        if ($cimPorts) {
            foreach ($dev in $cimPorts) {
                Write-Host "  [OK] $($dev.Caption)" -ForegroundColor Green
            }
            $portsFound = $true
        }
    } catch {
        # Fallback to basic port list
    }

    if (-not $portsFound) {
        try {
            $netPorts = [System.IO.Ports.SerialPort]::GetPortNames()
            if ($netPorts) {
                foreach ($port in $netPorts) {
                    Write-Host "  [OK] $port" -ForegroundColor Green
                }
                $portsFound = $true
            }
        } catch {}
    }

    if (-not $portsFound) {
        Write-Host "  [ERREUR] Aucun port serie COM detecte sur le systeme !" -ForegroundColor Red
        Write-Host "`nRecommandations de depannage :" -ForegroundColor White
        Write-Host "  - Verifiez que la carte WhisperEye est bien branchee en USB a votre PC." -ForegroundColor Gray
        Write-Host "  - Assurez-vous d'avoir installe les pilotes USB-to-UART requis (CP210x ou CH34x)." -ForegroundColor Gray
    }

    # 4. Troubleshooting Bootloader
    Write-Host "`nAstuce de Bootloader manuel (si le flash echoue ou ne repond pas) :" -ForegroundColor White
    Write-Host "  1. Maintenez le bouton BOOT (ou GPIO 0) de la carte enfonce." -ForegroundColor Gray
    Write-Host "  2. Appuyez brievement sur le bouton EN/RST." -ForegroundColor Gray
    Write-Host "  3. Relachez le bouton BOOT." -ForegroundColor Gray
    Write-Host "  4. Relancez la commande : .\we" -ForegroundColor Gray
    Write-Host "  5. Si la connexion echoue toujours, forcez une vitesse plus basse : .\we -B 115200" -ForegroundColor Gray
    Write-Host "----------------------------------------`n" -ForegroundColor Gray
}

# Parse custom port or baudrate from remaining arguments
$SelectedPort = $null
$BaudRate = $null
for ($i = 0; $i -lt $RemainingArgs.Count; $i++) {
    if ($RemainingArgs[$i] -eq "--port" -or $RemainingArgs[$i] -eq "-p") {
        if ($i + 1 -lt $RemainingArgs.Count) { $SelectedPort = $RemainingArgs[$i+1] }
    }
    elseif ($RemainingArgs[$i] -eq "--baud" -or $RemainingArgs[$i] -eq "-B") {
        if ($i + 1 -lt $RemainingArgs.Count) { $BaudRate = $RemainingArgs[$i+1] }
    }
}

try {
    # 5. Optional Clean
    if ($Clean) {
        Write-Host "[*] Cleaning cargo build artifacts..." -ForegroundColor Yellow
        cargo +esp clean
        if ($LASTEXITCODE -ne 0) {
            throw "cargo +esp clean a echoue avec le code de sortie $LASTEXITCODE"
        }
        
        # Supprimer le cache ESP-IDF partagé pour forcer la prise en compte de sdkconfig.defaults
        $sharedEspIdfCache = "C:\t\xtensa-esp32-espidf"
        if (Test-Path $sharedEspIdfCache) {
            Write-Host "[*] Nettoyage du cache ESP-IDF partagé : $sharedEspIdfCache..." -ForegroundColor Yellow
            Remove-Item -Recurse -Force $sharedEspIdfCache -ErrorAction SilentlyContinue
        }
    }

    # 6. Build, Flash & Monitor
    $flashArgs = @("flash", "--monitor")
    if ($Release) {
        $flashArgs += "--release"
        Write-Host "[*] Starting RELEASE build, flash and monitor..." -ForegroundColor Green
    } else {
        Write-Host "[*] Starting DEBUG build, flash and monitor..." -ForegroundColor Green
    }

    # S'assurer que la table de partition personnalisée est gérée de manière native par ESP-IDF (via sdkconfig.defaults)
    # On évite de passer --partition-table à espflash pour supporter le calcul automatique de l'espace restant (taille de partition vide).
    if ($RemainingArgs) {
        foreach ($arg in $RemainingArgs) {
            if ($arg -eq "--partition-table") {
                Write-Warning "[*] --partition-table specifie manuellement. Attention aux limitations d'espflash."
            }
        }
    }

    # Forward any remaining arguments directly to espflash
    if ($RemainingArgs) {
        $flashArgs += $RemainingArgs
    }

    Write-Host "[*] Running: cargo +esp espflash $flashArgs" -ForegroundColor Gray
    cargo +esp espflash $flashArgs
    
    # Check external command exit code
    if ($LASTEXITCODE -ne 0) {
        throw "cargo +esp espflash a echoue avec le code de sortie $LASTEXITCODE"
    }
}
catch {
    Write-Error "An error occurred during build/flash: $_"
    Show-Diagnostics -SelectedPort $SelectedPort -BaudRate $BaudRate
}
finally {
    Pop-Location
}
