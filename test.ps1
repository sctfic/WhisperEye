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
Write-Host "[*] Boot Delay: Sleeping for 10 seconds to allow firmware boot & Wi-Fi connection..." -ForegroundColor Yellow
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
        [bool]$IsRedirect = $false
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

    try {
        $res = Invoke-WebRequest @params
        $actualCode = $res.StatusCode
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
            } else {
                $errorMsg = "HTTP Error $actualCode"
            }
        } else {
            $errorMsg = $_.Exception.Message
        }
    }
    $duration = [Math]::Round(([DateTime]::Now - $startTime).TotalMilliseconds)

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

# ----------------- TEST EXECUTION -----------------

$passed = 0
$total = 0

# 1. Base Dashboard
$total++; if (Test-Endpoint "Main Dashboard" "GET" "/" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Favicon Resource" "GET" "/favicon.ico" -ExpectedStatusCodes @(200)) { $passed++ }

# 2. Captive Portal Redirects (Should return 302 Found)
$total++; if (Test-Endpoint "Captive Portal 204" "GET" "/generate_204" -ExpectedStatusCodes @(302) -IsRedirect $true) { $passed++ }
# $total++; if (Test-Endpoint "Captive Portal Hotspot" "GET" "/hotspot-detect.html" -ExpectedStatusCodes @(302) -IsRedirect $true) { $passed++ }
# $total++; if (Test-Endpoint "Captive Portal NCSI" "GET" "/ncsi.txt" -ExpectedStatusCodes @(302) -IsRedirect $true) { $passed++ }
# $total++; if (Test-Endpoint "Captive Portal ConnectTest" "GET" "/connecttest.txt" -ExpectedStatusCodes @(302) -IsRedirect $true) { $passed++ }

# 3. GET JSON API Endpoints
$total++; if (Test-Endpoint "System Status API" "GET" "/api/status" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Metrics History API" "GET" "/api/history" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Wi-Fi SSIDs Scan API" "GET" "/api/ssids" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Sensors Data API" "GET" "/api/sensors" -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Peripherals Display API" "GET" "/api/peripherals" -ExpectedStatusCodes @(200)) { $passed++ }

# Note: Check Updates API returns 400 Bad Request if no update URL is configured in NVS, which is correct behavior.
$total++; if (Test-Endpoint "Check Updates API" "GET" "/api/check_updates" -ExpectedStatusCodes @(200, 400)) { $passed++ }

# 4. POST API Endpoints
$total++; if (Test-Endpoint "Post Actuators API" "POST" "/api/actuators" -Body '{"rla": false, "rlb": false, "swpwr": true, "ina": false, "inb": false}' -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Post Peripherals Rename" "POST" "/api/peripherals" -Body '{"id": "rla", "name": "Relais A"}' -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Post Config (Apply Only)" "POST" "/api/config" -Body '{"auto_update": true}' -ExpectedStatusCodes @(200)) { $passed++ }
$total++; if (Test-Endpoint "Post Clear TOTP API" "POST" "/api/clear_totp" -ExpectedStatusCodes @(200)) { $passed++ }

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
[void][System.Console]::ReadKey($true)
