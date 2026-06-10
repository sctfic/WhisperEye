# WhisperEye API Test Suite
# Tests all endpoints of the WhisperEye production firmware

param(
    [string]$BaseUrl = "http://s3",
    [int]$delay = 10
)

# Premature exit safety
$ErrorActionPreference = "Continue"

# Premium console header
Clear-Host
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host "     WhisperEye Production API Integration Tests          " -ForegroundColor Cyan
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "[*] Boot Delay: Sleeping for $delay seconds to allow firmware boot & Wi-Fi connection..." -ForegroundColor Yellow
Start-Sleep -Seconds $delay
Write-Host "[*] Starting API validation on target: $BaseUrl" -ForegroundColor Gray
Write-Host ""

# Helper to run requests and handle results
function Test-Endpoint {
    param (
        [string]$Name,
        [string]$Method,
        [string]$Path,
        [string]$Body = $null,
        [int[]]$ExpectedStatusCodes = @(200),
        [bool]$IsRedirect = $false,
        [scriptblock]$Validator = $null
    )

    $Uri = "$BaseUrl$Path"
    $params = @{
        Method = $Method
        Uri = $Uri
        TimeoutSec = 5
        ErrorAction = "Stop"
    }

    if ($IsRedirect) {
        $params.MaximumRedirection = 0
    }

    if ($Body) {
        $params.Body = $Body
        $params.ContentType = "application/json"
    }

    $startTime = [DateTime]::Now
    $res = $null
    $success = $false
    $actualCode = 0
    $errorMsg = ""
    $content = $null

    try {
        $res = Invoke-WebRequest @params
        $actualCode = $res.StatusCode
        $content = $res.Content
        if ($ExpectedStatusCodes -contains $actualCode) {
            $success = $true
        } else {
            $errorMsg = "Unexpected status code: $actualCode (Expected: $($ExpectedStatusCodes -join ','))"
        }
    }
    catch {
        # Check if we got a response despite the error (e.g., 302 or 400 bad request)
        if ($_.Exception.Response) {
            $actualCode = [int]$_.Exception.Response.StatusCode
            if ($ExpectedStatusCodes -contains $actualCode) {
                $success = $true
                try {
                    $stream = $_.Exception.Response.GetResponseStream()
                    if ($null -ne $stream) {
                        $reader = New-Object System.IO.StreamReader($stream)
                        $content = $reader.ReadToEnd()
                        $reader.Close()
                        $stream.Close()
                    }
                } catch {}
            } else {
                $errorMsg = "HTTP Error $actualCode"
            }
        } else {
            $errorMsg = $_.Exception.Message
        }
    }
    $duration = [Math]::Round(([DateTime]::Now - $startTime).TotalMilliseconds)

    if ($success -and $null -ne $Validator -and $null -ne $content) {
        try {
            $trimmed = $content.Trim()
            $isJson = $trimmed.StartsWith("{") -or $trimmed.StartsWith("[")
            if ($isJson) {
                $parsed = $content | ConvertFrom-Json
                $validationResult = & $Validator $parsed $actualCode
            } else {
                $validationResult = & $Validator $content $actualCode
            }

            if ($validationResult -is [bool] -and -not $validationResult) {
                $success = $false
                $errorMsg = "Format validation failed"
            } elseif ($validationResult -is [string] -and $validationResult -ne "OK") {
                $success = $false
                $errorMsg = "Format validation failed: $validationResult"
            }
        }
        catch {
            $success = $false
            $errorMsg = "Exception during response parsing/validation: $($_.Exception.Message)"
        }
    }

    if ($success) {
        Write-Host "  [OK] " -NoNewline -ForegroundColor Green
        Write-Host "$($Name.PadRight(30)) | $Method $Path | Code $actualCode | ${duration}ms" -ForegroundColor Gray
        return $true
    } else {
        Write-Host "  [FAIL] " -NoNewline -ForegroundColor Red
        Write-Host "$($Name.PadRight(30)) | $Method $Path | Error: $errorMsg" -ForegroundColor Yellow
        return $false
    }
}

# ----------------- RESPONSE FORMAT VALIDATORS -----------------

$statusValidator = {
    param($j)
    if ($null -eq $j) { return "JSON is null" }
    $reqFields = @("network_mode", "wifi_ssid", "ip_addr", "gateway_addr", "sys_time", "ntp_server", "fw_version", "wifi_known", "auto_update", "has_totp", "author")
    foreach ($f in $reqFields) {
        if ($null -eq $j.$f) { return "Missing field: $f" }
    }
    if ($null -eq $j.author.email) { return "Missing field: author.email" }
    return "OK"
}

$capacityValidator = {
    param($j)
    if ($null -eq $j) { return "JSON is null" }
    if ($null -eq $j.sensors) { return "Missing field: sensors" }
    if ($null -eq $j.actuators) { return "Missing field: actuators" }
    $sensors = @($j.sensors)
    foreach ($s in $sensors) {
        if ($null -eq $s.Unit) { return "Missing field: Unit in sensor $($s.Name)" }
    }
    return "OK"
}

$historyValidator = {
    param($j)
    $items = @($j)
    foreach ($item in $items) {
        if ($null -eq $item.timestamp) { return "Missing field: timestamp in history entry" }
        if ($null -eq $item.readings) { return "Missing field: readings in history entry" }
        if ($null -eq $item.readings.temperature_sht45) { return "Missing field: readings.temperature_sht45" }
        if ($null -eq $item.readings.co2_scd41) { return "Missing field: readings.co2_scd41" }
    }
    return "OK"
}

$ssidsValidator = {
    param($j)
    if ($null -eq $j) { return "JSON is null" }
    if ($null -eq $j.ssids) { return "Missing field: ssids" }
    if ($null -eq $j.active) { return "Missing field: active" }
    return "OK"
}

$sensorsValidator = {
    param($j)
    if ($null -eq $j) { return "JSON is null" }
    $reqFields = @("temperature_sht45", "humidity_sht45", "co2_scd41", "ds18b20_temperatures")
    foreach ($f in $reqFields) {
        if ($null -eq $j.$f) { return "Missing field: $f" }
    }
    return "OK"
}

$peripheralsValidator = {
    param($j)
    $items = @($j)
    if ($items.Count -eq 0) { return "OK" }
    $first = $items[0]
    $reqFields = @("id", "name", "is_static", "present", "value")
    foreach ($f in $reqFields) {
        if ($null -eq $first.$f) { return "Missing field: $f in peripherals entry" }
    }
    return "OK"
}

$checkUpdatesValidator = {
    param($data, $statusCode)
    if ($statusCode -eq 400) {
        if ($data -like "*No update URL configured*") { return "OK" }
        return "Expected 'No update URL configured' error message"
    }
    if ($null -eq $data.boardType) { return "Missing boardType" }
    if ($null -eq $data.ChipType) { return "Missing ChipType" }
    return "OK"
}

# ----------------- TEST EXECUTION -----------------

$passed = 0
$total = 0

# 1. Base Dashboard
$total++; if (Test-Endpoint "Main Dashboard" "GET" "/" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Favicon Resource" "GET" "/favicon.ico" -ExpectedStatusCodes @(200)) { $passed++ }

# 2. Captive Portal Redirects (Should return 302 Found)
$total++; if (Test-Endpoint "Captive Portal 204" "GET" "/generate_204" -ExpectedStatusCodes @(302) -IsRedirect $true) { $passed++ }

# 3. GET JSON API Endpoints
$total++; if (Test-Endpoint "System Status API" "GET" "/api/status" -ExpectedStatusCodes @(200) -Validator $statusValidator) { $passed++ }
$total++; if (Test-Endpoint "Capacity API" "GET" "/api/capacity" -ExpectedStatusCodes @(200) -Validator $capacityValidator) { $passed++ }
$total++; if (Test-Endpoint "Metrics History API" "GET" "/api/history" -ExpectedStatusCodes @(200) -Validator $historyValidator) { $passed++ }
$total++; if (Test-Endpoint "Wi-Fi SSIDs Scan API" "GET" "/api/ssids" -ExpectedStatusCodes @(200) -Validator $ssidsValidator) { $passed++ }
$total++; if (Test-Endpoint "Sensors Data API" "GET" "/api/sensors" -ExpectedStatusCodes @(200) -Validator $sensorsValidator) { $passed++ }
$total++; if (Test-Endpoint "Peripherals Display API" "GET" "/api/peripherals" -ExpectedStatusCodes @(200) -Validator $peripheralsValidator) { $passed++ }
$total++; if (Test-Endpoint "Check Updates API" "GET" "/api/check_updates" -ExpectedStatusCodes @(200, 400) -Validator $checkUpdatesValidator) { $passed++ }

# 4. POST API Endpoints
$total++; if (Test-Endpoint "Post Actuators API" "POST" "/api/actuators" -Body '{"rla": false, "rlb": false, "swpwr": true, "ina": false, "inb": false}' -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Post Peripherals Rename" "POST" "/api/peripherals" -Body '{"id": "rla", "name": "Relais A"}' -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Post Config (Apply Only)" "POST" "/api/config" -Body '{"auto_update": true}' -ExpectedStatusCodes @(200)) { $passed++ }

Write-Host ""
Write-Host "==========================================================" -ForegroundColor Cyan
if ($passed -eq $total) {
    Write-Host "    TEST RESULT: PASS ($passed/$total tests successful) " -ForegroundColor Green
} else {
    Write-Host "    TEST RESULT: FAIL ($passed/$total tests successful) " -ForegroundColor Red
}
Write-Host "==========================================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Press any key to exit..." -ForegroundColor Gray
if ($Host.Name -eq "ConsoleHost") {
    [void][System.Console]::ReadKey($true)
}

