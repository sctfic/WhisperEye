# PowerShell Utility script for WhisperEye Workspace compilation and upload
param (
    [Parameter(Mandatory = $false)]
    [ValidateSet("factory", "production")]
    [string]$Target = "factory",

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
$env:LDPROXY_LINKER = "xtensa-esp32-elf-gcc"
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

# 2. Setup targets
$Package = if ($Target -eq "factory") { "factory_boot" } else { "production_app" }
$BuildProfile = if ($Debug) { "debug" } else { "release" }
$ProfileFlag = if ($Debug) { "" } else { "--release" }

# 3. Clean Cache
if ($Clean) {
    Write-Host "[*] Cleaning Cargo cache..." -ForegroundColor Gray
    cargo +esp clean
}

# 4. Compile
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


# 5. Flash Upload
Write-Host "[*] Initiating upload process for $Package..." -ForegroundColor Cyan
$FlashCommand = "cargo +esp espflash flash --package $Package --partition-table partitions.csv"
if (-not $Debug) {
    $FlashCommand += " --release"
}
if ($Port) {
    $FlashCommand += " --port $Port"
}
$FlashCommand += " --monitor"

Write-Host "    -> Invoking command: $FlashCommand" -ForegroundColor DarkGray

try {
    # Run the flash command directly in the shell
    Invoke-Expression $FlashCommand
}
catch {
    # Failures are handled below
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
    Write-Host "• Microcontrôleur Cible : ESP32 (ESP-WROOM-32)" -ForegroundColor Gray
    Write-Host "• Target Rust           : xtensa-esp32-espidf" -ForegroundColor Gray
    Write-Host "• Package sélectionné   : $Package" -ForegroundColor Gray
    Write-Host "• Fichier de partitions : partitions.csv (Factory: 2MB, Production: 2MB)" -ForegroundColor Gray
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
