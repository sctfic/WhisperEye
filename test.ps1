# WhisperEye API Test Suite using Pester
# Tests all endpoints of the WhisperEye production firmware

param(
    [string]$BaseUrl = "http://192.168.1.101",
    [int]$delay = 10
)

# Premature exit safety
$ErrorActionPreference = "Continue"

# If we are not running inside Pester (no Describe command defined in scope),
# we invoke Pester on this file itself!
if ($null -eq (Get-Command -Name "Describe" -ErrorAction SilentlyContinue)) {
    # Load Pester module
    Import-Module Pester -ErrorAction SilentlyContinue
    
    # Premium console header
    Clear-Host
    Write-Host "==========================================================" -ForegroundColor Cyan
    Write-Host "     WhisperEye Production API Pester Tests Runner        " -ForegroundColor Cyan
    Write-Host "==========================================================" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "[*] Boot Delay: Sleeping for $delay seconds to allow firmware boot & Wi-Fi connection..." -ForegroundColor Yellow
    Start-Sleep -Seconds $delay
    Write-Host "[*] Executing Pester tests on target: $BaseUrl" -ForegroundColor Gray
    Write-Host ""
    
    Invoke-Pester -Script @{ Path = $PSCommandPath; Parameters = @{ BaseUrl = $BaseUrl } }
    
    Write-Host ""
    Write-Host "Press any key to exit..." -ForegroundColor Gray
    if ($Host.Name -eq "ConsoleHost") {
        [void][System.Console]::ReadKey($true)
    }
    Exit
}

# Helper to query HTTP endpoints and return status & body (even for redirects/errors)
function Get-HttpStatus {
    param(
        [string]$Url,
        [string]$Method = "GET",
        $Body = $null
    )
    $params = @{
        Uri = $Url
        Method = $Method
        TimeoutSec = 5
        ErrorAction = "Stop"
        MaximumRedirection = 0
        UseBasicParsing = $true
    }
    if ($Body) {
        $params.Body = $Body
        $params.ContentType = "application/json"
    }
    try {
        $res = Invoke-WebRequest @params
        return [PSCustomObject]@{
            StatusCode = $res.StatusCode
            Content = $res.Content
        }
    } catch {
        if ($_.TargetObject -and $_.TargetObject -is [System.Net.HttpWebRequest]) {
            try {
                $resp = $_.TargetObject.GetResponse()
                $stream = $resp.GetResponseStream()
                $bodyText = ""
                if ($null -ne $stream -and $stream.CanRead) {
                    $reader = New-Object System.IO.StreamReader($stream)
                    $bodyText = $reader.ReadToEnd()
                    $reader.Close()
                    $stream.Close()
                }
                $statusCode = [int]$resp.StatusCode
                $resp.Close()
                return [PSCustomObject]@{
                    StatusCode = $statusCode
                    Content = $bodyText
                }
            } catch {
                # Fall through
            }
        }
        if ($_.Exception.Response) {
            $stream = $_.Exception.Response.GetResponseStream()
            $bodyText = ""
            if ($null -ne $stream -and $stream.CanRead) {
                $reader = New-Object System.IO.StreamReader($stream)
                $bodyText = $reader.ReadToEnd()
                $reader.Close()
                $stream.Close()
            }
            return [PSCustomObject]@{
                StatusCode = [int]$_.Exception.Response.StatusCode
                Content = $bodyText
            }
        }
        throw $_
    }
}

# ----------------- PESTER TEST SUITE -----------------
Describe "WhisperEye Production API Integration" {

    Context "Base Dashboard Resources" {
        It "should load the Main Dashboard (GET /)" {
            $res = Get-HttpStatus "$BaseUrl/"
            $res.StatusCode | Should Be 200
        }

        It "should load the Favicon Resource (GET /favicon.ico)" {
            $res = Get-HttpStatus "$BaseUrl/favicon.ico"
            $res.StatusCode | Should Be 200
        }
    }

    Context "Captive Portal Redirects" {
        It "should redirect Android/Chrome to captive portal (GET /generate_204)" {
            $res = Get-HttpStatus "$BaseUrl/generate_204"
            $res.StatusCode | Should Be 302
        }

        It "should redirect iOS/Apple to captive portal (GET /hotspot-detect.html)" {
            $res = Get-HttpStatus "$BaseUrl/hotspot-detect.html"
            $res.StatusCode | Should Be 302
        }

        It "should redirect Windows NCSI to captive portal (GET /ncsi.txt)" {
            $res = Get-HttpStatus "$BaseUrl/ncsi.txt"
            $res.StatusCode | Should Be 302
        }

        It "should redirect Windows ConnectTest to captive portal (GET /connecttest.txt)" {
            $res = Get-HttpStatus "$BaseUrl/connecttest.txt"
            $res.StatusCode | Should Be 302
        }
    }

    Context "GET JSON API Endpoints" {
        It "should return System Status details (GET /api/status)" {
            $res = Get-HttpStatus "$BaseUrl/api/status"
            $res.StatusCode | Should Be 200
            
            $j = $res.Content | ConvertFrom-Json
            $j.network_mode | Should Not BeNullOrEmpty
            $j.wifi_ssid | Should Not BeNullOrEmpty
            $j.ip_addr | Should Not BeNullOrEmpty
            $j.gateway_addr | Should Not BeNullOrEmpty
            $j.sys_time | Should Not BeNullOrEmpty
            # ntp_server may be empty before NTP sync
            ($j.PSObject.Properties.Name -contains "ntp_server") | Should Be $true
            $j.fw_version | Should Not BeNullOrEmpty
            $j.wifi_known | Should Not Be $null
            # Booleans: $false would fail BeNullOrEmpty, use Not Be $null instead
            $j.auto_update | Should Not Be $null
            $j.has_totp | Should Not Be $null
            $j.author.email | Should Not BeNullOrEmpty
        }

        It "should return Peripherals capacity properties (GET /api/capacity)" {
            $res = Get-HttpStatus "$BaseUrl/api/capacity"
            $res.StatusCode | Should Be 200

            $j = $res.Content | ConvertFrom-Json
            $j.sensors | Should Not BeNullOrEmpty
            $j.actuators | Should Not BeNullOrEmpty
            
            $sensors = @($j.sensors)
            foreach ($s in $sensors) {
                $s.Unit | Should Not BeNullOrEmpty
            }
        }

        It "should return metrics history list (GET /api/history)" {
            $res = Get-HttpStatus "$BaseUrl/api/history"
            $res.StatusCode | Should Be 200

            $j = $res.Content | ConvertFrom-Json
            $items = @($j)
            foreach ($item in $items) {
                $item.timestamp | Should Not BeNullOrEmpty
                $item.readings | Should Not BeNullOrEmpty
                $item.readings.temperature_sht45 | Should Not BeNullOrEmpty
                $item.readings.co2_scd41 | Should Not BeNullOrEmpty
            }
        }

        It "should return scanned Wi-Fi SSID networks (GET /api/ssids)" {
            $res = Get-HttpStatus "$BaseUrl/api/ssids"
            $res.StatusCode | Should Be 200

            $j = $res.Content | ConvertFrom-Json
            $j.ssids | Should Not BeNullOrEmpty
            $j.active | Should Not BeNullOrEmpty
        }

        It "should return live sensors readings (GET /api/sensors)" {
            $res = Get-HttpStatus "$BaseUrl/api/sensors"
            $res.StatusCode | Should Be 200

            $j = $res.Content | ConvertFrom-Json
            $j.temperature_sht45 | Should Not BeNullOrEmpty
            $j.humidity_sht45 | Should Not BeNullOrEmpty
            $j.co2_scd41 | Should Not BeNullOrEmpty
            $j.ds18b20_temperatures | Should Not BeNullOrEmpty
        }

        It "should return registered peripherals details (GET /api/peripherals)" {
            $res = Get-HttpStatus "$BaseUrl/api/peripherals"
            $res.StatusCode | Should Be 200

            $j = $res.Content | ConvertFrom-Json
            $items = @($j)
            if ($items.Count -gt 0) {
                $first = $items[0]
                $first.id | Should Not BeNullOrEmpty
                $first.name | Should Not BeNullOrEmpty
                $first.is_static | Should Not BeNullOrEmpty
                $first.present | Should Not BeNullOrEmpty
                $first.value | Should Not BeNullOrEmpty
            }
        }

        # Possible responses:
        #   400 - No updateAvailable URL configured in NVS
        #   502 - Upstream firmware manifest server unreachable
        #   200 - Returns matched board entry JSON (or null if no board match)
        It "should check updates or return 400/502 when update URL not configured or unreachable (GET /api/check_updates)" {
            $res = Get-HttpStatus "$BaseUrl/api/check_updates"
            if ($res.StatusCode -eq 400) {
                $res.Content | Should Like "*No update URL configured*"
            } elseif ($res.StatusCode -eq 502) {
                $res.Content | Should Like "*Upstream error*"
            } else {
                $res.StatusCode | Should Be 200
                # Response may be null if no board matches the local boardType/ChipType
                if ($res.Content -ne "null") {
                    $j = $res.Content | ConvertFrom-Json
                    $j.boardType | Should Not BeNullOrEmpty
                    $j.ChipType | Should Not BeNullOrEmpty
                }
            }
        }
    }

    Context "POST Mutation API Endpoints" {
        It "should toggle relay outputs (POST /api/actuators)" {
            $body = '{"rla": false, "rlb": false, "swpwr": true, "ina": false, "inb": false}'
            $res = Get-HttpStatus "$BaseUrl/api/actuators" -Method POST -Body $body
            $res.StatusCode | Should Be 200
        }

        It "should rename a peripheral device (POST /api/peripherals)" {
            $body = '{"id": "rla", "name": "Relais A"}'
            $res = Get-HttpStatus "$BaseUrl/api/peripherals" -Method POST -Body $body
            $res.StatusCode | Should Be 200
        }

        It "should update system config settings (POST /api/config)" {
            $body = '{"auto_update": true}'
            $res = Get-HttpStatus "$BaseUrl/api/config" -Method POST -Body $body
            $res.StatusCode | Should Be 200
        }

        It "should reject reset without correct confirmation (POST /api/reset)" {
            $body = '{"confirm": "WRONG"}'
            $res = Get-HttpStatus "$BaseUrl/api/reset" -Method POST -Body $body
            $res.StatusCode | Should Be 400
        }
    }
}
