# monitoronly.ps1 — Serial Monitor for ESP32-S3 WhisperEye
# Usage: .\monitoronly.ps1 [COM10]   (port optionnel, auto-détection sinon)
param (
    [Parameter(Mandatory = $false)]
    [string]$Port = ""
)

$ErrorActionPreference = "Stop"
$BaudRate = 115200

Clear-Host
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "     WhisperEye Serial Monitor — ESP32-S3 Logs Viewer     " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan

# 1. Source ESP-IDF environment (needed for idf.py monitor)
$EspExportScript = "C:\Users\Alban\export-esp.ps1"
$HasEspIdf = $false
if (Test-Path $EspExportScript) {
    Write-Host "[*] Sourcing Espressif Toolchain: $EspExportScript..." -ForegroundColor Gray
    . $EspExportScript
    if (Get-Command idf.py -ErrorAction SilentlyContinue) {
        $HasEspIdf = $true
    }
} else {
    Write-Host "[!] export-esp.ps1 introuvable. Mode PowerShell brut." -ForegroundColor Yellow
}

# 2. Detect COM port if not provided
if (-not $Port) {
    Write-Host "[*] Aucun port specifie. Detection automatique des ports COM..." -ForegroundColor Gray
    
    $cimPorts = Get-CimInstance Win32_PnPEntity -ErrorAction SilentlyContinue |
        Where-Object { $_.Caption -match 'COM(\d+)' -and $_.Caption -notmatch 'Bluetooth' } |
        ForEach-Object {
            if ($_.Caption -match 'COM(\d+)') {
                [PSCustomObject]@{
                    Port    = "COM$($Matches[1])"
                    Name    = $_.Caption
                    ComNumber = [int]$Matches[1]
                }
            }
        } |
        Sort-Object ComNumber

    if (-not $cimPorts -or $cimPorts.Count -eq 0) {
        Write-Host "[-] Aucun port COM non-Bluetooth detecte !" -ForegroundColor Red
        Write-Host "    Verifiez le branchement USB et les pilotes (CP210x / CH340)." -ForegroundColor Gray
        Write-Host "    Utilisez : .\monitoronly.ps1 COMx pour forcer un port." -ForegroundColor Gray
        exit 1
    }

    Write-Host "[+] Ports COM detectes (hors Bluetooth) :" -ForegroundColor Green
    foreach ($p in $cimPorts) {
        Write-Host "    $($p.Port)  —  $($p.Name)" -ForegroundColor Gray
    }

    # Prefer the lowest COM number (usually the first ESP32 plugged in)
    $Port = $cimPorts[0].Port
    Write-Host "[*] Port selectionne automatiquement : $Port" -ForegroundColor Cyan
} else {
    # Normalize: ensure COM prefix
    if ($Port -notmatch "^COM") {
        $Port = "COM$Port"
    }
    $Port = $Port.ToUpper()
}

Write-Host "[*] Ouverture du moniteur serie sur $Port a $BaudRate bauds..." -ForegroundColor Cyan
Write-Host "[*] Appuyez sur Ctrl+C pour quitter." -ForegroundColor DarkGray
Write-Host "==========================================================" -ForegroundColor DarkGray

# 3. Try idf.py monitor first (best experience with colors & timestamps)
if ($HasEspIdf) {
    Write-Host "[*] Lancement via idf.py monitor..." -ForegroundColor Gray
    $env:ESPTOOL_PORT = $Port
    try {
        idf.py monitor --port $Port
    } catch {
        Write-Host "[!] idf.py monitor a echoue, fallback PowerShell..." -ForegroundColor Yellow
        $HasEspIdf = $false
    }
}

# 4. Fallback: PowerShell native serial reader
if (-not $HasEspIdf) {
    $serial = $null
    try {
        $serial = New-Object System.IO.Ports.SerialPort $Port, $BaudRate, None, 8, One
        $serial.ReadTimeout = 1000
        $serial.Open()
        Write-Host "[+] Connecte a $Port. Lecture des logs..." -ForegroundColor Green
        Write-Host "==========================================================" -ForegroundColor DarkGray
        
        while ($true) {
            try {
                $line = $serial.ReadLine()
                # Colorize: errors in red, warnings in yellow
                if ($line -match '^\s*[EW]\s*\(') {
                    Write-Host $line -ForegroundColor Red
                } elseif ($line -match '^\s*W\s*\(') {
                    Write-Host $line -ForegroundColor Yellow
                } elseif ($line -match '\x1b\[3[0-7]m') {
                    # Already has ANSI colors, just output
                    Write-Host $line
                } else {
                    Write-Host $line -ForegroundColor Gray
                }
            } catch [TimeoutException] {
                continue
            }
        }
    } catch {
        Write-Host "[-] Erreur port serie : $_" -ForegroundColor Red
        Write-Host "[-] Le port $Port est peut-etre deja ouvert ou inexistant." -ForegroundColor Red
        exit 1
    } finally {
        if ($serial -and $serial.IsOpen) {
            $serial.Close()
            Write-Host "[*] Port serie ferme." -ForegroundColor Gray
        }
    }
}
